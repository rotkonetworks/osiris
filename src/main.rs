// #![recursion_limit = "256"]
// mod handler;
// pub use handler::Handler;

// mod responder;
// pub use responder::Responder;

// mod trader;
// pub use trader::Trader;

// mod opt;
// pub use opt::{gather_history, Opt};

// mod wallet;
// pub use wallet::Wallet;

// mod catchup;
// pub use catchup::Catchup;

// #[tokio::main]
// async fn main() -> anyhow::Result<()> {
//     tracing_subscriber::fmt::init();

use std::sync::atomic::AtomicBool;

//     use clap::Parser;
//     Opt::parse().exec().await
// }
use binance::config::Config;
use binance::websockets::WebSockets;
use binance::websockets::WebsocketEvent;

fn main() {
    let keep_running = AtomicBool::new(true); // Used to control the event loop
                                              // let kline = format!("{}", "ethbtc@kline_1m");
                                              // let book_ticker = format!("{}", "ethbtc@bookTicker");
    let endpoints =
        ["ETHBTC", "ETHUSD"].map(|symbol| format!("{}@bookTicker", symbol.to_lowercase()));

    let mut web_socket = WebSockets::new(|event: WebsocketEvent| {
        match event {
            WebsocketEvent::BookTicker(book_ticker_event) => {
                println!(
                    "Symbol: {}, best_bid: {}, best_ask: {}",
                    book_ticker_event.symbol,
                    book_ticker_event.best_bid,
                    book_ticker_event.best_ask
                );
            }
            _ => (),
        };
        Ok(())
    });

    web_socket
        .connect_with_config(
            &format!("stream?streams={}", &endpoints.join("/")),
            &Config {
                rest_api_endpoint: "https://api.binance.us".into(),
                // ws_endpoint: "wss://stream.binance.us:9443/ws".into(),
                ws_endpoint: "wss://stream.binance.us:9443".into(),
                ..Default::default()
            },
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
}
