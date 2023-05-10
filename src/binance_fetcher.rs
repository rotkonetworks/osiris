use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;

use binance::model::BookTickerEvent;
use tokio::sync::watch::Sender;

use binance::config::Config;
use binance::websockets::WebSockets;
use binance::websockets::WebsocketEvent;

/// The `Trader` maps `(DirectedTradingPair, Amount)` position requests to [`position::Id`] identifiers of opened positions.
pub struct BinanceFetcher {
    /// Sends quotes to the trader.
    txs: BTreeMap<String, Sender<Option<BookTickerEvent>>>,
    /// Keep track of last-sent prices to avoid spamming the trader.
    last_sent: BTreeMap<String, BookTickerEvent>,
    /// The symbols we are fetching.
    symbols: Vec<String>,
    /// The configuration for the Binance API client
    binance_config: Config,
}

impl BinanceFetcher {
    pub fn new(
        txs: BTreeMap<String, Sender<Option<BookTickerEvent>>>,
        symbols: Vec<String>,
        binance_config: Config,
    ) -> Self {
        Self {
            txs,
            last_sent: BTreeMap::new(),
            symbols,
            binance_config,
        }
    }

    /// Run the fetcher.
    pub async fn run(mut self) -> anyhow::Result<()> {
        tracing::debug!("running BinanceFetcher");

        let keep_running = AtomicBool::new(true); // Used to control the event loop
        let endpoints = self
            .symbols
            .iter()
            .map(|symbol| format!("{}@bookTicker", symbol.to_lowercase()))
            .collect::<Vec<_>>();

        let mut web_socket = WebSockets::new(|event: WebsocketEvent| {
            match event {
                WebsocketEvent::BookTicker(book_ticker_event) => {
                    // Check if this quote has already been sent or not.
                    let last_sent_quote = self.last_sent.get(&book_ticker_event.symbol);

                    if last_sent_quote.is_none()
                        || (last_sent_quote.is_some()
                            && last_sent_quote.unwrap().update_id < book_ticker_event.update_id &&
                            // we actually only care to update iff the price has changed
                            (last_sent_quote.unwrap().best_bid != book_ticker_event.best_bid ||
                            last_sent_quote.unwrap().best_ask != book_ticker_event.best_ask))
                    {
                        tracing::debug!(?book_ticker_event, "received new quote");
                        self.txs
                            .get(&book_ticker_event.symbol)
                            .expect("missing sender for symbol")
                            .send(Some(book_ticker_event.clone()))
                            .expect("error sending price quote");
                        self.last_sent
                            .insert(book_ticker_event.symbol.clone(), book_ticker_event);
                    }
                }
                _ => (),
            };
            Ok(())
        });

        web_socket
            .connect_with_config(
                &format!("stream?streams={}", &endpoints.join("/")),
                &self.binance_config,
            )
            .unwrap(); // check error
        if let Err(e) = web_socket.event_loop(&keep_running) {
            match e {
                err => {
                    println!("Error: {:?}", err);
                }
            }
        }
        web_socket.disconnect().unwrap();
        Ok(())
    }
}
