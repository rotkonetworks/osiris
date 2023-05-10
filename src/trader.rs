use std::{collections::BTreeMap, pin::Pin, sync::Arc};

use binance::model::BookTickerEvent;
use futures::{Future, FutureExt};
use penumbra_crypto::{asset, keys::AddressIndex, FullViewingKey};
use penumbra_custody::CustodyClient;
use penumbra_view::ViewClient;
use tokio::sync::watch;

#[derive(Clone)]
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
        println!("run run run");

        // let mut self2 = Arc::new(self.clone());

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
                        let notes = self
                            .view
                            .unspent_notes_by_address_and_asset(self.fvk.account_group_id())
                            .await?;
                        // // let notes = Arc::get_mut(&mut self2).unwrap().get_balance().await?;
                        println!("notes: {:?}", notes.get(&AddressIndex::from(self.account)));
                    }
                }
            }
        }

        Ok(())
    }
}
