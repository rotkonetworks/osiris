use penumbra_crypto::Address;
use regex::Regex;
use tokio::sync::oneshot;

use super::Response;

#[derive(Debug, Clone)]
pub struct PriceQuote {
    pub asset: String,
    pub best_ask: f64,
    pub best_bid: f64,
}

/// A request to be fulfilled by the responder service.
#[derive(Debug, Clone)]
pub struct Request {
    /// The price quote.
    pub quote: PriceQuote,
}

impl Request {
    // /// Create a new request by scanning the contents of a [`Message`].
    // ///
    // /// Returns a receiver for the response to this request, as well as the request itself.
    // pub fn try_new(message: &Message) -> Option<(oneshot::Receiver<Response>, Request)> {
    //     let address_regex =
    //         Regex::new(r"penumbrav\dt1[qpzry9x8gf2tvdw0s3jn54khce6mua7l]*").unwrap();

    //     // Collect all the matches into a struct, bundled with the original message
    //     tracing::trace!("collecting addresses from message");
    //     let addresses: Vec<AddressOrAlmost> = address_regex
    //         .find_iter(&message.content)
    //         .map(|m| {
    //             use AddressOrAlmost::*;
    //             match m.as_str().parse() {
    //                 Ok(addr) => Address(Box::new(addr)),
    //                 Err(e) => {
    //                     tracing::trace!(error = ?e, "failed to parse address");
    //                     Almost(m.as_str().to_string())
    //                 }
    //             }
    //         })
    //         .collect();

    //     // If no addresses were found, don't bother sending the message to the queue
    //     if addresses.is_empty() {
    //         None
    //     } else {
    //         let (tx, rx) = oneshot::channel();
    //         Some((
    //             rx,
    //             Request {
    //                 addresses,
    //                 response: tx,
    //             },
    //         ))
    //     }
    // }
}
