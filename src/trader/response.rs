use std::fmt::Write;

use penumbra_crypto::Address;
use penumbra_transaction::Id;

/// The response from a request to dispense tokens to a set of addresses.
#[derive(Debug)]
pub struct Response {
    /// The addresses that were successfully dispensed tokens
    pub(super) succeeded: Vec<(Address, Id)>,
    /// The addresses that failed to be dispensed tokens, accompanied by a string describing the
    /// error.
    pub(super) failed: Vec<(Address, String)>,
    /// The addresses that couldn't be parsed.
    pub(super) unparsed: Vec<String>,
    /// The addresses that were limited from being dispensed tokens because only a certain number
    /// are permitted to be given tokens per message.
    pub(super) remaining: Vec<Address>,
}

impl Response {
    /// Returns the addresses that were successfully dispensed tokens.
    pub fn succeeded(&self) -> &[(Address, Id)] {
        &self.succeeded
    }

    /// Returns the addresses that failed to be dispensed tokens, accompanied by a string describing
    /// the error.
    pub fn failed(&self) -> &[(Address, String)] {
        &self.failed
    }

    /// Returns the addresses that couldn't be parsed.
    pub fn unparsed(&self) -> &[String] {
        &self.unparsed
    }

    /// Returns the addresses that were limited from being dispensed tokens because only a certain
    /// number are permitted to be given tokens per message.
    pub fn remaining(&self) -> &[Address] {
        &self.remaining
    }

    /// Returns `true` only if all addresses were successfully dispensed tokens.
    pub fn complete_success(&self) -> bool {
        self.failed.is_empty() && self.unparsed.is_empty() && self.remaining.is_empty()
    }

    /// Returns `false` only if no addresses were successfully dispensed tokens.
    pub fn complete_failure(&self) -> bool {
        self.succeeded.is_empty() && !self.complete_success()
    }
}
