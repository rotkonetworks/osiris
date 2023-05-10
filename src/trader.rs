use std::{collections::BTreeMap, str::FromStr};

use binance::model::BookTickerEvent;
use penumbra_crypto::{
    asset::Denom,
    dex::{lp::position, Market},
    keys::AddressIndex,
    Amount, Fee, FullViewingKey,
};
use penumbra_custody::{AuthorizeRequest, CustodyClient};
use penumbra_view::{Planner, ViewClient};
use rand::rngs::OsRng;
use tokio::sync::watch;

use lazy_static::lazy_static;

// Mapping the symbol (in Binance's API, a `String` like `ETHBTC`) to
// a Penumbra `Market` is tricky.
//
// Luckily, since we only care about a handful of symbols, we can just
// hardcode the mapping here. If you were trying to work with other symbols,
// you'd crash and hopefully find this constant!
lazy_static! {
    pub static ref SYMBOL_MAP: BTreeMap<String, Market> = BTreeMap::from([
        (
            // ETH priced in terms of BTC
            "ETHBTC".to_string(),
            Market::from_str("test_btc:test_eth").unwrap()
        ),
        (
            // ETH priced in terms of USD
            "ETHUSD".to_string(),
            Market::from_str("test_usd:test_eth").unwrap()
        ),
        (
            // BTC priced in terms of USD
            "BTCUSD".to_string(),
            Market::from_str("test_usd:test_btc").unwrap()
        ),
        (
            // ATOM priced in terms of BTC
            "ATOMBTC".to_string(),
            Market::from_str("test_btc:test_atom").unwrap()
        ),
        (
            // ATOM priced in terms of USD
            "ATOMUSD".to_string(),
            Market::from_str("test_usd:test_atom").unwrap()
        ),
    ]);
}

pub struct Trader<V, C>
where
    V: ViewClient + Clone + Send + 'static,
    C: CustodyClient + Clone + Send + 'static,
{
    /// Actions to perform.
    // actions: BTreeMap<String, Arc<watch::Receiver<Option<BookTickerEvent>>>>,
    actions: BTreeMap<String, watch::Receiver<Option<BookTickerEvent>>>,
    view: V,
    custody: C,
    fvk: FullViewingKey,
    account: u32,
}

impl<V, C> Trader<V, C>
where
    V: ViewClient + Clone + Send + 'static,
    C: CustodyClient + Clone + Send + 'static,
{
    /// Create a new trader.
    pub fn new(
        account: u32,
        fvk: FullViewingKey,
        view: V,
        custody: C,
        // List of symbols to monitor
        symbols: Vec<String>,
    ) -> (
        BTreeMap<String, watch::Sender<Option<BookTickerEvent>>>,
        Self,
    ) {
        // Construct a tx/rx pair for each symbol we're tracking
        let mut txs = BTreeMap::new();
        let mut rxs = BTreeMap::new();
        for symbol in symbols {
            let (tx, rx) = watch::channel(None);
            txs.insert(symbol.clone(), tx);
            // rxs.insert(symbol.clone(), Arc::new(rx));
            rxs.insert(symbol.clone(), rx);
        }
        (
            txs,
            Trader {
                // binance_fetcher,
                actions: rxs,
                view,
                custody,
                fvk,
                account,
            },
        )
    }

    /// Run the responder.
    pub async fn run(mut self) -> anyhow::Result<()> {
        loop {
            // Check each pair
            for (symbol, rx) in self.actions.iter() {
                // If there's a new event, process it
                if rx.has_changed().unwrap() {
                    let bte = rx.clone().borrow_and_update().clone();
                    if let Some(book_ticker_event) = bte {
                        tracing::debug!("trader received event: {:?}", book_ticker_event);
                        println!(
                            "Symbol: {}, best_bid: {}, best_ask: {}",
                            book_ticker_event.symbol,
                            book_ticker_event.best_bid,
                            book_ticker_event.best_ask
                        );

                        // Get the current balance from the view service.
                        // We could do this outside of the loop, but checking here
                        // assures we have the latest data, and the in-memory gRPC interface
                        // should be fast.
                        let notes = self
                            .view
                            .unspent_notes_by_address_and_asset(self.fvk.account_group_id())
                            .await?;

                        // Find the balance we have for the two assets in the market.
                        if let Some(notes) = notes.get(&AddressIndex::from(self.account)) {
                            let market = SYMBOL_MAP
                                .get(symbol)
                                .expect("missing symbol -> Market mapping");

                            let (asset_1, asset_2) = (market.start.clone(), market.end.clone());

                            let (mut reserves_1, mut reserves_2) =
                                (Amount::from(0u32), Amount::from(0u32));

                            for (asset_id, note_records) in notes {
                                if *asset_id == asset_1.id() {
                                    for note_record in note_records {
                                        reserves_1 += note_record.note.amount();
                                    }
                                }
                                if *asset_id == asset_2.id() {
                                    for note_record in note_records {
                                        reserves_2 += note_record.note.amount();
                                    }
                                }
                            }

                            tracing::debug!(
                                ?asset_1,
                                ?reserves_1,
                                ?asset_2,
                                ?reserves_2,
                                "found balance for assets"
                            );
                        }

                        // See what liquidity positions we currently have open
                        // in both directions of the market.
                        fn is_opened_position_nft(denom: &Denom) -> bool {
                            let prefix = format!("lpnft_opened_");

                            tracing::debug!(?denom, "checking if denom is an opened position NFT");
                            denom.starts_with(&prefix)
                        }

                        let asset_cache = self.view.assets().await?;

                        // Create a `Vec<String>` of the currently closed LPs
                        // for this trading pair so we can withdraw them.
                        //
                        // Their reserves can be used when opening the new position
                        // in the transaction.

                        // Create a `Vec<String>` of the currently open LPs
                        // for this trading pair so we can close them.
                        let lp_open_notes: Vec<String> = notes
                            .iter()
                            .flat_map(|(index, notes_by_asset)| {
                                // Include each note individually:
                                notes_by_asset.iter().flat_map(|(_asset, notes)| {
                                    notes
                                        .iter()
                                        .filter(|record| {
                                            let base_denom =
                                                asset_cache.get(&record.note.asset_id());
                                            if base_denom.is_none() {
                                                return false;
                                            }

                                            // TODO: ensure the LPNFT is for the correct market!
                                            // currently this will try to close all LPs, lol
                                            (*index == AddressIndex::from(self.account))
                                                && is_opened_position_nft(base_denom.unwrap())
                                        })
                                        .map(|record| {
                                            asset_cache
                                                .get(&record.note.asset_id())
                                                .unwrap()
                                                .to_string()
                                        })
                                })
                            })
                            .collect();

                        // Pay no fee for the close transaction.
                        let fee = Fee::from_staking_token_amount(0u32.into());

                        let mut plan = &mut Planner::new(OsRng);

                        if lp_open_notes.is_empty() {
                            tracing::debug!("No open positions are available to close.");
                        } else {
                            for denom_string in lp_open_notes {
                                // Close the position associated with the opened NFT note
                                let position_id = position::Id::from_str(&denom_string[13..])
                                    .expect("improperly formatted LPNFT position id");
                                plan = plan.position_close(position_id);
                            }
                        }

                        let final_plan = plan
                            .fee(fee)
                            .plan(
                                &mut self.view,
                                self.fvk.account_group_id(),
                                AddressIndex::from(self.account),
                            )
                            .await;

                        // Sometimes building the plan can fail with an error, because there were no actions
                        // present. There's not an easy way to check this in the planner API right now.
                        if let Err(e) = final_plan {
                            tracing::debug!(?e, "failed to build plan");
                            continue;
                        }

                        let final_plan = final_plan.unwrap();

                        // 2. Authorize and build the transaction.
                        let auth_data = self
                            .custody
                            .authorize(AuthorizeRequest {
                                plan: final_plan.clone(),
                                account_group_id: Some(self.fvk.account_group_id()),
                                pre_authorizations: Vec::new(),
                            })
                            .await?
                            .data
                            .ok_or_else(|| anyhow::anyhow!("no auth data"))?
                            .try_into()?;
                        let witness_data = self
                            .view
                            .witness(self.fvk.account_group_id(), &final_plan)
                            .await?;
                        let unauth_tx = final_plan
                            .build_concurrent(OsRng, &self.fvk, witness_data)
                            .await?;

                        let tx = unauth_tx.authorize(&mut OsRng, &auth_data)?;

                        // 3. Broadcast the transaction and wait for confirmation.
                        self.view.broadcast_transaction(tx, true).await?;
                    }
                }
            }
        }

        Ok(())
    }
}
