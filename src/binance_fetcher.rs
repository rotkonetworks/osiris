use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use tokio::time::{sleep, Duration};
use std::sync::Arc;
use binance::model::BookTickerEvent;
use tokio::sync::watch::Sender;
use binance::config::Config;
use binance::websockets::{WebSockets, WebsocketEvent};
use parking_lot::RwLock;

#[derive(Debug)]
pub struct BinanceFetcher {
    txs: BTreeMap<String, Sender<Option<BookTickerEvent>>>,
    last_sent: BTreeMap<String, BookTickerEvent>,
    symbols: Vec<String>,
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

    pub async fn run(self) -> anyhow::Result<()> {
        tracing::info!("starting binance fetcher");
        let keep_running = Arc::new(AtomicBool::new(true));
        let endpoints = self
            .symbols
            .iter()
            .map(|symbol| format!("{}@bookTicker", symbol.to_lowercase()))
            .collect::<Vec<_>>();

        let max_retries = 5;
        let mut retry_count = 0;
        let mut backoff_duration = Duration::from_secs(1);

        while keep_running.load(std::sync::atomic::Ordering::Relaxed) {
            match self.connect_and_stream(&endpoints, Arc::clone(&keep_running)).await {
                Ok(_) => {
                    retry_count = 0;
                    backoff_duration = Duration::from_secs(1);
                }
                Err(e) => {
                    retry_count += 1;
                    if retry_count >= max_retries {
                        tracing::error!(?e, "max retries exceeded, shutting down fetcher");
                        return Err(e);
                    }
                    tracing::warn!(
                        ?e,
                        ?retry_count,
                        ?backoff_duration,
                        "websocket error, attempting reconnect"
                    );
                    sleep(backoff_duration).await;
                    backoff_duration = std::cmp::min(backoff_duration * 2, Duration::from_secs(32));
                }
            }
        }
        Ok(())
    }

    async fn connect_and_stream(
        &self,
        endpoints: &[String],
        keep_running: Arc<AtomicBool>,
    ) -> anyhow::Result<()> {
        struct CallbackState {
            last_sent: Arc<RwLock<BTreeMap<String, BookTickerEvent>>>,
            txs: Arc<BTreeMap<String, Sender<Option<BookTickerEvent>>>>,
        }

        unsafe impl Send for CallbackState {}
        unsafe impl Sync for CallbackState {}

        let state = Arc::new(CallbackState {
            last_sent: Arc::new(RwLock::new(self.last_sent.clone())),
            txs: Arc::new(self.txs.clone()),
        });

        let state_clone = Arc::clone(&state);

        let callback = Box::new(move |event| {
            let state = &state_clone;
            if let WebsocketEvent::BookTicker(book_ticker_event) = event {
                let mut last_sent = state.last_sent.write();
                let last_sent_quote = last_sent.get(&book_ticker_event.symbol);

                if last_sent_quote.is_none()
                    || (last_sent_quote.is_some()
                        && last_sent_quote.unwrap().update_id < book_ticker_event.update_id
                        && (last_sent_quote.unwrap().best_bid != book_ticker_event.best_bid
                            || last_sent_quote.unwrap().best_ask != book_ticker_event.best_ask))
                {
                    tracing::debug!(?book_ticker_event, ?last_sent_quote, "received new quote");
                    if let Some(tx) = state.txs.get(&book_ticker_event.symbol) {
                        if let Err(e) = tx.send(Some(book_ticker_event.clone())) {
                            tracing::error!("Failed to send price quote: {}", e);
                        }
                    }
                    last_sent.insert(book_ticker_event.symbol.clone(), book_ticker_event);
                }
            }
            Ok(())
        }) as Box<dyn FnMut(WebsocketEvent) -> Result<(), binance::errors::Error> + Send>;

        let mut web_socket = WebSockets::new(callback);

        tracing::debug!(?endpoints, "connecting to Binance websocket API");
        web_socket
            .connect_with_config(
                &format!("stream?streams={}", &endpoints.join("/")),
                &self.binance_config,
            )
            .map_err(|e| anyhow::anyhow!("failed to connect to binance websocket service: {e}"))?;

        web_socket
            .event_loop(&keep_running)
            .map_err(|e| anyhow::anyhow!("WebSocket event loop error: {}", e))?;

        Ok(())
    }
}
