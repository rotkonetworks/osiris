use std::{collections::BTreeMap, future, str::FromStr};

use anyhow::Context;
use binance::model::BookTickerEvent;
use futures::{StreamExt, TryStreamExt};
use penumbra_crypto::{asset::DenomMetadata, keys::AddressIndex, Amount, Fee, FullViewingKey};
use penumbra_custody::{AuthorizeRequest, CustodyClient};
use penumbra_dex::{
    lp::{
        position::{self, Position},
        Reserves,
    },
    DirectedUnitPair,
};
use penumbra_proto::client::v1alpha1::LiquidityPositionsByPriceRequest;
use penumbra_proto::client::v1alpha1::{
    specific_query_service_client::SpecificQueryServiceClient, LiquidityPositionsRequest,
};
use penumbra_view::{Planner, ViewClient};
use rand::rngs::OsRng;
use tokio::sync::watch;
use tonic::transport::{Channel, ClientTlsConfig};

use lazy_static::lazy_static;
use url::Url;

// Mapping the symbol (in Binance's API, a `String` like `ETHBTC`) to
// a Penumbra `DirectedUnitPair` is tricky.
//
// Luckily, since we only care about a handful of symbols, we can just
// hardcode the mapping here. If you were trying to work with other symbols,
// you'd crash and hopefully find this constant!
lazy_static! {
    pub static ref SYMBOL_MAP: BTreeMap<String, DirectedUnitPair> = BTreeMap::from([
        (
            // ETH priced in terms of BTC
            "ETHBTC".to_string(),
            DirectedUnitPair::from_str("test_btc:test_eth").unwrap()
        ),
        (
            // ETH priced in terms of USD
            "ETHUSD".to_string(),
            DirectedUnitPair::from_str("test_usd:test_eth").unwrap()
        ),
        (
            // BTC priced in terms of USD
            "BTCUSD".to_string(),
            DirectedUnitPair::from_str("test_usd:test_btc").unwrap()
        ),
        (
            // ATOM priced in terms of BTC
            "ATOMBTC".to_string(),
            DirectedUnitPair::from_str("test_btc:test_atom").unwrap()
        ),
        (
            // ATOM priced in terms of USD
            "ATOMUSD".to_string(),
            DirectedUnitPair::from_str("test_usd:test_atom").unwrap()
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
    last_updated_height: BTreeMap<String, u64>,
    view: V,
    custody: C,
    fvk: FullViewingKey,
    account: u32,
    pd_url: Url,
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
        pd_url: Url,
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
                pd_url,
                last_updated_height: BTreeMap::new(),
            },
        )
    }

    /// Run the trader.
    pub async fn run(mut self) -> anyhow::Result<()> {
        // Doing this loop without any shutdown signal doesn't exactly
        // provide a clean shutdown, but it works for now.
        loop {
            // TODO: ensure we have some positions from `penumbra` to create a more interesting
            // trading environment :)

            // Check each pair
            let mut actions = self.actions.clone();
            for (symbol, rx) in actions.iter_mut() {
                // If there's a new quote for this symbol, process it
                if rx.has_changed().unwrap() {
                    let bte = rx.borrow_and_update().clone();
                    if bte.is_none() {
                        continue;
                    }
                    let book_ticker_event = bte.unwrap();
                    tracing::debug!("trader received event: {:?}", book_ticker_event);

                    // Only update positions for a given symbol at most once per block
                    let current_height = self
                        .view
                        .status(self.fvk.account_group_id())
                        .await?
                        .sync_height;
                    if let Some(last_updated_height) = self.last_updated_height.get(symbol) {
                        if !(current_height > *last_updated_height) {
                            tracing::debug!(?symbol, "skipping symbol, already updated this block");
                            continue;
                        }
                    }

                    println!(
                        "Symbol: {}, best_bid: {}, best_ask: {}",
                        book_ticker_event.symbol,
                        book_ticker_event.best_bid,
                        book_ticker_event.best_ask
                    );

                    // Look up the Binance symbol's mapping to a Penumbra market
                    let market = SYMBOL_MAP
                        .get(symbol)
                        .expect("missing symbol -> DirectedUnitPair mapping");

                    // Create a plan that will contain all LP management operations based on this quote.
                    // TODO: could move this outside the loop, but it's a little easier to debug
                    // the plans like this for now
                    let plan = &mut Planner::new(OsRng);

                    // Find the spendable balance for each asset in the market.
                    // This only counts the spendable notes associated with the assets,
                    // later on we will also need to account for the liquidity positions
                    // being withdrawn.
                    let (mut reserves_1, mut reserves_2) =
                        self.get_spendable_balance(market).await?;

                    // Fetch all known open liquidity positions for the trading pair (both directions).
                    let open_liquidity_positions: Vec<Position> =
                        self.get_owned_open_liquidity_positions(market).await?;

                    // Close all our the open liquidity positions for the trading pair (both directions).
                    self.close_liquidity_positions(open_liquidity_positions, plan)
                        .await?;

                    // Fetch all our closed liquidity positions for the trading pair (both directions).
                    let closed_liquidity_positions: Vec<Position> =
                        self.get_owned_closed_liquidity_positions(market).await?;

                    // Since the closed liquidity positions will be withdrawn in this transaction, we can use their reserves
                    // when opening the new liquidity positions.
                    for position in closed_liquidity_positions.iter() {
                        // The canonical ordering in the position may differ from the market directed ordering
                        if position.phi.pair.asset_1() == market.start.id() {
                            reserves_1 += position.reserves.r1;
                            reserves_2 += position.reserves.r2;
                        } else {
                            reserves_1 += position.reserves.r2;
                            reserves_2 += position.reserves.r1;
                        }
                    }

                    // Withdraw all closed liquidity positions for this trading pair.
                    self.withdraw_liquidity_positions(closed_liquidity_positions, plan)
                        .await?;

                    // Open liquidity position with half of the reserves we have available
                    self.open_liquidity_position(
                        market,
                        reserves_1,
                        reserves_2,
                        book_ticker_event.clone(),
                        plan,
                    )
                    .await?;

                    // TODO: it's possible to immediately close this position within the same block
                    // however what if we don't get updates every block?

                    // Finalize and submit the transaction plan.
                    match self.finalize_and_submit(plan).await {
                        Ok(_) => {}
                        Err(e) => {
                            tracing::error!(
                                ?e,
                                ?market,
                                ?current_height,
                                ?book_ticker_event,
                                ?plan,
                                "failed to update position"
                            );
                            continue;
                        }
                    };

                    // Update the last updated height for this symbol
                    self.last_updated_height
                        .insert(symbol.clone(), current_height);

                    tracing::info!(
                        ?market,
                        ?current_height,
                        ?book_ticker_event,
                        "successfully updated positions"
                    );
                }
            }
            self.actions = actions;
        }

        Ok(())
    }

    async fn finalize_and_submit(&mut self, plan: &mut Planner<OsRng>) -> anyhow::Result<()> {
        // Pay no fee for the transaction.
        let fee = Fee::from_staking_token_amount(0u32.into());

        // Sometimes building the plan can fail with an error, because there were no actions
        // present. There's not an easy way to check this in the planner API right now.
        let final_plan = plan
            .fee(fee)
            .plan(
                &mut self.view,
                self.fvk.account_group_id(),
                AddressIndex::from(self.account),
            )
            .await?;

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

        Ok(())
    }

    async fn close_liquidity_positions(
        &mut self,
        positions: Vec<Position>,
        mut plan: &mut Planner<OsRng>,
    ) -> anyhow::Result<()> {
        if positions.is_empty() {
            tracing::debug!("No open positions are available to close.");
        } else {
            for pos in positions {
                // Close the position
                tracing::trace!(?pos, "closing position");
                plan = plan.position_close(pos.id());
            }
        }

        Ok(())
    }

    async fn open_liquidity_position(
        &mut self,
        market: &DirectedUnitPair,
        reserves_1: Amount,
        reserves_2: Amount,
        quote: BookTickerEvent,
        mut plan: &mut Planner<OsRng>,
    ) -> anyhow::Result<()> {
        // put up half the available reserves
        // TODO: this isn't quite right as it's half the _remaining_ reserves
        // and there might be multiple positions involving reserves with the same
        // assets. so we should really have _half the total available reserves divided
        // amongst the positions involving that asset_
        let reserves = Reserves {
            r1: reserves_1 / 2u32.into(),
            r2: reserves_2 / 2u32.into(),
        };

        if reserves.r1 == 0u32.into() && reserves.r2 == 0u32.into() {
            // No reserves available to open a position.
            tracing::debug!(?market, "No reserves available to open a position.");
            return Ok(());
        }

        // The `fee` here is the `spread`.
        let best_ask = quote
            .best_ask
            .parse::<f64>()
            .expect("bad f64 in binance api return");
        let best_bid = quote
            .best_bid
            .parse::<f64>()
            .expect("bad f64 in binance api return");

        // a 1_000scaling factor is applied to keep some of the decimal
        // TODO: this is bad lol
        let mut scaling_factor = 1_000.0;
        let mut mid_price = scaling_factor * (best_ask + best_bid) / 2.0;

        // apply more scaling factor if necessary
        while mid_price < 1.0 {
            scaling_factor *= 1_000.0;
            mid_price *= 1_000.0;
        }

        // Calculate spread:
        let difference = scaling_factor * (best_ask - best_bid).abs();
        let fraction = difference / mid_price;
        // max of 50% fee, min of 100 bps (1%)
        let spread = (fraction * 100.0 * 100.0).clamp(100.0, 5000.0);

        let numer_scaler = market.start.unit_amount().value();
        let denom_scaler = market.end.unit_amount().value();

        let pos = Position::new(
            OsRng,
            market.into_directed_trading_pair(),
            spread as u32,
            // p is always the scaling value
            (scaling_factor as u128 * denom_scaler).into(),
            // price is expressed in units of asset 2
            (mid_price as u128 * numer_scaler).into(),
            reserves,
        );

        tracing::trace!(?pos, "opening position");
        plan = plan.position_open(pos);

        Ok(())
    }

    async fn withdraw_liquidity_positions(
        &mut self,
        positions: Vec<Position>,
        mut plan: &mut Planner<OsRng>,
    ) -> anyhow::Result<()> {
        if positions.is_empty() {
            tracing::debug!("No closed positions are available to withdraw.");
        } else {
            for pos in positions {
                // Withdraw the position
                let position_id = pos.id();
                tracing::trace!(?pos, "withdrawing position");
                plan = plan.position_withdraw(position_id, pos.reserves, pos.phi.pair);
            }
        }

        Ok(())
    }

    /// Returns only closed liquidity positions that we own.
    async fn get_owned_closed_liquidity_positions(
        &mut self,
        market: &DirectedUnitPair,
    ) -> anyhow::Result<Vec<Position>> {
        // We need to use the list of our notes to determine which positions we own.
        let notes = self
            .view
            .unspent_notes_by_address_and_asset(self.fvk.account_group_id())
            .await?;

        fn is_closed_position_nft(denom: &DenomMetadata) -> bool {
            let prefix = format!("lpnft_closed_");

            denom.starts_with(&prefix)
        }

        // TODO: we query the view server for this too much, maybe
        // we should hang on to it somewhere
        let asset_cache = self.view.assets().await?;

        // Create a `Vec<String>` of the currently closed LPs
        // for all trading pairs.
        let lp_closed_notes: Vec<String> = notes
            .iter()
            .flat_map(|(index, notes_by_asset)| {
                // Include each note individually:
                notes_by_asset.iter().flat_map(|(_asset, notes)| {
                    notes
                        .iter()
                        .filter(|record| {
                            let base_denom = asset_cache.get(&record.note.asset_id());
                            if base_denom.is_none() {
                                return false;
                            }

                            (*index == AddressIndex::from(self.account))
                                && is_closed_position_nft(base_denom.unwrap())
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

        let closed_liquidity_positions: Vec<Position> = self
            .get_closed_liquidity_positions(market)
            .await?
            .iter()
            // Exclude positions we don't control
            // by seeing if we own a closed LPNFT for the position
            .filter(|pos| lp_closed_notes.contains(&format!("lpnft_closed_{}", &pos.id())))
            .cloned()
            .collect::<Vec<_>>();

        tracing::debug!(
            ?closed_liquidity_positions,
            "found owned closed liquidity positions"
        );
        Ok(closed_liquidity_positions)
    }

    /// Returns only open liquidity positions that we own.
    async fn get_owned_open_liquidity_positions(
        &mut self,
        market: &DirectedUnitPair,
    ) -> anyhow::Result<Vec<Position>> {
        // We need to use the list of our notes to determine which positions we own.
        let notes = self
            .view
            .unspent_notes_by_address_and_asset(self.fvk.account_group_id())
            .await?;

        fn is_opened_position_nft(denom: &DenomMetadata) -> bool {
            let prefix = format!("lpnft_opened_");

            denom.starts_with(&prefix)
        }

        // TODO: we query the view server for this too much, maybe
        // we should hang on to it somewhere
        let asset_cache = self.view.assets().await?;

        // Create a `Vec<String>` of the currently open LPs
        // for all trading pairs.
        let lp_open_notes: Vec<String> = notes
            .iter()
            .flat_map(|(index, notes_by_asset)| {
                // Include each note individually:
                notes_by_asset.iter().flat_map(|(_asset, notes)| {
                    notes
                        .iter()
                        .filter(|record| {
                            let base_denom = asset_cache.get(&record.note.asset_id());
                            if base_denom.is_none() {
                                return false;
                            }

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
        tracing::debug!(?lp_open_notes, "found owned open LP notes");

        // TODO: we could query the chain for each liquidity position we own an open NFT for
        // and see if it's for this market. instead we iterate every single position. oh well, works
        // for now. ideally the view service would handle this better.
        let open_liquidity_positions: Vec<Position> = self
            .get_open_liquidity_positions(market)
            .await?
            .iter()
            // Exclude positions we don't control
            // by seeing if we own an open LPNFT for the position
            .filter(|pos| lp_open_notes.contains(&format!("lpnft_opened_{}", &pos.id())))
            .cloned()
            .collect::<Vec<_>>();

        tracing::debug!(
            ?open_liquidity_positions,
            "found owned open liquidity positions"
        );
        Ok(open_liquidity_positions)
    }

    /// Returns _all_ closed liquidity positions for both directions of the market,
    /// whether they're owned by us or not.
    async fn get_closed_liquidity_positions(
        &self,
        market: &DirectedUnitPair,
    ) -> anyhow::Result<Vec<Position>> {
        let mut client = self.specific_client().await?;

        // There's no queryable index for non-open positions,
        // so we need to query _all_ known positions and then filter :(
        // TODO: maybe support this better in pd?
        // https://github.com/penumbra-zone/penumbra/issues/2528
        let positions_stream =
            // This API will return positions of any status.
            client.liquidity_positions(LiquidityPositionsRequest {
                include_closed: true,
                ..Default::default()
            });
        let positions_stream = positions_stream.await?.into_inner();

        let positions = positions_stream
            .map_err(|e| anyhow::anyhow!("error fetching liquidity positions: {}", e))
            .and_then(|msg| async move {
                msg.data
                    .ok_or_else(|| anyhow::anyhow!("missing liquidity position in response data"))
                    .map(Position::try_from)?
            })
            .boxed()
            // filter for closed positions for the market we want
            .filter(|pos| {
                future::ready(
                    pos.as_ref().unwrap().phi.pair == market.into_directed_trading_pair().into()
                        && pos.as_ref().unwrap().state == position::State::Closed,
                )
            })
            .try_collect::<Vec<_>>()
            .await?;

        tracing::debug!(?positions, "found closed liquidity positions");
        Ok(positions)
    }

    /// Returns _all_ open liquidity positions for both directions of the market,
    /// whether they're owned by us or not.
    async fn get_open_liquidity_positions(
        &self,
        market: &DirectedUnitPair,
    ) -> anyhow::Result<Vec<Position>> {
        let mut client = self.specific_client().await?;

        // Check forward direction:
        let positions_stream =
            // This API will return only open positions.
            client.liquidity_positions_by_price(LiquidityPositionsByPriceRequest {
                trading_pair: Some(market.into_directed_trading_pair().into()),
                limit: 0,
                ..Default::default()
            });
        let positions_stream = positions_stream.await?.into_inner();

        let forward_positions = positions_stream
            .map_err(|e| anyhow::anyhow!("error fetching liquidity positions: {}", e))
            .and_then(|msg| async move {
                msg.data
                    .ok_or_else(|| anyhow::anyhow!("missing liquidity position in response data"))
                    .map(Position::try_from)?
            })
            .boxed()
            .try_collect::<Vec<_>>()
            .await?;

        // Check flipped direction:
        let positions_stream =
            // This API will return only open positions.
            client.liquidity_positions_by_price(LiquidityPositionsByPriceRequest {
                trading_pair: Some(market.into_directed_trading_pair().flip().into()),
                limit: 0,
                ..Default::default()
            });
        let positions_stream = positions_stream.await?.into_inner();

        let flipped_positions = positions_stream
            .map_err(|e| anyhow::anyhow!("error fetching liquidity positions: {}", e))
            .and_then(|msg| async move {
                msg.data
                    .ok_or_else(|| anyhow::anyhow!("missing liquidity position in response data"))
                    .map(Position::try_from)?
            })
            .boxed()
            .try_collect::<Vec<_>>()
            .await?;

        let mut positions = vec![];
        positions.extend(forward_positions);
        positions.extend(flipped_positions);

        // A position might have been returned twice, so dedupe:
        let mut deduped_positions: Vec<Position> = vec![];
        'outer: for pos in positions {
            for p2 in &deduped_positions {
                if pos.id() == p2.id() {
                    continue 'outer;
                }
            }
            deduped_positions.push(pos);
        }

        tracing::debug!(?deduped_positions, "found open liquidity positions");
        Ok(deduped_positions)
    }

    async fn get_spendable_balance(
        &mut self,
        market: &DirectedUnitPair,
    ) -> anyhow::Result<(Amount, Amount)> {
        // Get the current balance from the view service.
        // We could do this outside of the loop, but checking here
        // assures we have the latest data, and the in-memory gRPC interface
        // should be fast.
        let notes = self
            .view
            .unspent_notes_by_address_and_asset(self.fvk.account_group_id())
            .await?;

        let (mut reserves_1, mut reserves_2) = (Amount::from(0u32), Amount::from(0u32));

        // Find the balance we have for the two assets in the market.
        if let Some(notes) = notes.get(&AddressIndex::from(self.account)) {
            let (asset_1, asset_2) = (market.start.clone(), market.end.clone());

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

        Ok((reserves_1, reserves_2))
    }

    async fn pd_channel(&self) -> anyhow::Result<Channel> {
        match self.pd_url.scheme() {
            "http" => Ok(Channel::from_shared(self.pd_url.to_string())?
                .connect()
                .await?),
            "https" => Ok(Channel::from_shared(self.pd_url.to_string())?
                .tls_config(ClientTlsConfig::new())?
                .connect()
                .await?),
            other => Err(anyhow::anyhow!("unknown url scheme {other}"))
                .with_context(|| format!("could not connect to {}", self.pd_url)),
        }
    }

    pub async fn specific_client(
        &self,
    ) -> Result<SpecificQueryServiceClient<Channel>, anyhow::Error> {
        let channel = self.pd_channel().await?;
        Ok(SpecificQueryServiceClient::new(channel))
    }
}
