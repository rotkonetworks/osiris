use std::{
    borrow::Borrow,
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
};

use binance::model::BookTickerEvent;
use penumbra_crypto::FullViewingKey;
use penumbra_custody::CustodyClient;
use penumbra_view::ViewClient;
use tokio::{
    sync::watch,
    time::{Duration, Instant},
};
use tracing::instrument;

mod request;
pub use request::{PriceQuote, Request};

mod response;
pub use response::Response;

pub struct Trader<V, C>
where
    V: ViewClient + Clone + Send + 'static,
    C: CustodyClient + Clone + Send + 'static,
{
    /// Actions to perform.
    /// TODO: there should be a mapping of pairs to receivers, otherwise
    /// it's possible for most-recent quotes to be missed depending on
    /// the order in which quotes are transmitted, since we are using a `tokio::sync::watch`.
    actions: BTreeMap<String, Arc<watch::Receiver<Option<BookTickerEvent>>>>,
    view: V,
    custody: C,
    fvk: FullViewingKey,
    account: u32,
    // /// Responsible for submitting transactions to Penumbra.
    // binance_fetcher: BinanceFetcher<V, C>,
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
            rxs.insert(symbol.clone(), Arc::new(rx));
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
        loop {
            // Check each pair
            for (symbol, mut rx) in self.actions.iter_mut() {
                // If there's a new event, process it
                if rx.has_changed().unwrap() {
                    if let Some(book_ticker_event) = Arc::get_mut(&mut rx)
                        .expect("one rx ref")
                        .borrow_and_update()
                        .clone()
                    {
                        println!("trader received event: {:?}", book_ticker_event);

                        println!(
                            "Symbol: {}, best_bid: {}, best_ask: {}",
                            book_ticker_event.symbol,
                            book_ticker_event.best_bid,
                            book_ticker_event.best_ask
                        );
                    }
                }
            }
            // while self.actions.has_changed().is_ok() {
            //     if self.actions.has_changed().unwrap() {
            //         println!("actions has changed");
            //         if let Some(book_ticker_event) = self.actions.borrow_and_update().clone() {
            //             println!("trader received event: {:?}", book_ticker_event);

            //             println!(
            //                 "Symbol: {}, best_bid: {}, best_ask: {}",
            //                 book_ticker_event.symbol,
            //                 book_ticker_event.best_bid,
            //                 book_ticker_event.best_ask
            //             );
            //         }
            //     }
            // }
        }
        println!("not ok i guess lol");

        Ok(())
    }

    // /// Try to dispense tokens to the given addresses, collecting [`Response`] describing what
    // /// happened.
    // async fn dispense(&mut self, mut addresses: Vec<AddressOrAlmost>) -> anyhow::Result<Response> {
    //     // Track addresses to which we successfully dispensed tokens
    //     let mut succeeded = Vec::<(Address, Id)>::new();

    //     // Track addresses (and associated errors) which we tried to send tokens to, but failed
    //     let mut failed = Vec::<(Address, String)>::new();

    //     // Track addresses which couldn't be parsed
    //     let mut unparsed = Vec::<String>::new();

    //     // Extract up to the maximum number of permissible valid addresses from the list
    //     let mut count = 0;
    //     while count <= self.max_addresses {
    //         count += 1;
    //         match addresses.pop() {
    //             Some(AddressOrAlmost::Address(addr)) => {
    //                 // Reply to the originating message with the address
    //                 let span = tracing::info_span!("send", address = %addr);
    //                 span.in_scope(|| {
    //                     tracing::info!("processing send request, waiting for readiness");
    //                 });
    //                 let rsp = self
    //                     .trader
    //                     .ready()
    //                     .await?
    //                     .call((*addr, self.values.clone()))
    //                     .instrument(span.clone());
    //                 tracing::info!("submitted send request");

    //                 match rsp.await {
    //                     Ok(id) => {
    //                         span.in_scope(|| {
    //                             tracing::info!(id = %id, "send request succeeded");
    //                         });
    //                         succeeded.push((*addr, id));
    //                     }
    //                     // By default, anyhow::Error's Display impl only prints the outermost error;
    //                     // using the alternate formate specifier prints the entire chain of causes.
    //                     Err(e) => failed.push((*addr, format!("{:#}", e))),
    //                 }
    //             }
    //             Some(AddressOrAlmost::Almost(addr)) => {
    //                 unparsed.push(addr);
    //             }
    //             None => break,
    //         }
    //     }

    //     // Separate the rest of the list into unparsed and remaining valid ones
    //     let mut remaining = Vec::<Address>::new();
    //     for addr in addresses {
    //         match addr {
    //             AddressOrAlmost::Address(addr) => remaining.push(*addr),
    //             AddressOrAlmost::Almost(addr) => unparsed.push(addr),
    //         }
    //     }

    //     Ok(Response {
    //         succeeded,
    //         failed,
    //         unparsed,
    //         remaining,
    //     })
    // }
}

// impl BinanceFetcher {
//     async fn message(&self, ctx: Context, message: Message) {
//         tracing::trace!("parsing message: {:#?}", message);
//         // Get the guild id of this message
//         let guild_id = if let Some(guild_id) = message.guild_id {
//             guild_id
//         } else {
//             return;
//         };

//         // Get the channel of this message
//         let guild_channel = if let Some(guild_channel) = ctx.cache.guild_channel(message.channel_id)
//         {
//             guild_channel
//         } else {
//             tracing::trace!("could not find server");
//             return;
//         };

//         let self_id = ctx.cache.current_user().id;
//         let user_id = message.author.id;
//         let user_name = message.author.name.clone();

//         // Stop if we're not allowed to respond in this channel
//         if let Ok(self_permissions) = guild_channel.permissions_for_user(&ctx, self_id) {
//             if !self_permissions.send_messages() {
//                 tracing::trace!(
//                     ?guild_channel,
//                     "not allowed to send messages in this channel"
//                 );
//                 return;
//             }
//         } else {
//             return;
//         };

//         // Don't trigger on messages we ourselves send
//         if user_id == self_id {
//             tracing::trace!("detected message from ourselves");
//             return;
//         }

//         // Prune the send history of all expired rate limit timeouts
//         {
//             tracing::trace!("pruning send history");
//             // scoped to prevent deadlock on send_history
//             let mut send_history = self.send_history.lock().unwrap();
//             while let Some((user, last_fulfilled, _)) = send_history.front() {
//                 if last_fulfilled.elapsed() >= self.rate_limit {
//                     tracing::debug!(?user, ?last_fulfilled, "rate limit expired");
//                     send_history.pop_front();
//                 } else {
//                     break;
//                 }
//             }
//             tracing::trace!("finished pruning send history");
//         }

//         // Check if the message contains a penumbra address and create a request for it if so
//         let (response, request) = if let Some(parsed) = { Request::try_new(&message) } {
//             parsed
//         } else {
//             tracing::trace!("no addresses found in message");
//             return;
//         };

//         // If the message author was in the send history, don't send them tokens
//         let rate_limited = self
//             .send_history
//             .lock()
//             .unwrap()
//             .iter_mut()
//             .find(|(user, _, _)| *user == user_id)
//             .map(|(_, last_fulfilled, notified)| {
//                 // Increase the notification count by one and return the previous count:
//                 let old_notified = *notified;
//                 *notified += 1;
//                 (*last_fulfilled, old_notified)
//             });

//         if let Some((last_fulfilled, notified)) = rate_limited {
//             tracing::info!(
//                 ?user_name,
//                 ?notified,
//                 user_id = ?user_id.to_string(),
//                 ?last_fulfilled,
//                 "rate-limited user"
//             );

//             // If we already notified the user, don't reply again
//             if notified > self.reply_limit + 1 {
//                 return;
//             }

//             let response = format!(
//                 "Please wait for another {} before requesting more tokens. Thanks!",
//                 format_remaining_time(last_fulfilled, self.rate_limit)
//             );
//             reply(&ctx, &message, response).await;

//             // Setting the notified count to zero "un-rate-limits" an entry, which we do when a
//             // request fails, so we don't have to traverse the entire list:
//             if notified > 0 {
//                 // So therefore we only prevent the request when the notification count is greater
//                 // than zero
//                 return;
//             }
//         }

//         // Push the user into the send history queue for rate-limiting in the future
//         tracing::trace!(?user_name, user_id = ?user_id.to_string(), "pushing user into send history");
//         self.send_history
//             .lock()
//             .unwrap()
//             .push_back((user_id, Instant::now(), 1));

//         // Send the message to the queue, to be processed asynchronously
//         tracing::trace!("sending message to worker queue");
//         ctx.data
//             .read()
//             .await
//             .get::<RequestQueue>()
//             .expect("address queue exists")
//             .send(request)
//             .await
//             .expect("send to queue always succeeds");
//     }
// }
