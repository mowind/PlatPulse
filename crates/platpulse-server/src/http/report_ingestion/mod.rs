//! Transactional AgentReport ingestion for the first Inventory vertical slice.
//!
//! The stages of one Report live in their own modules: [`boundary`] admits the
//! request, [`ingest`] runs the transaction, [`receipt`] builds the receipt or
//! rejection, [`clock`] places observations on the Server clock, [`projection`],
//! [`peers`] and [`block_history`] write what the Report observed, [`network`]
//! checks Network Identity and the Observed Network Head, and [`transfer`]
//! resolves Node transfers between Agents.

mod block_history;
mod boundary;
mod clock;
mod ingest;
mod network;
mod peers;
mod projection;
mod receipt;
#[cfg(test)]
mod test_helpers;
mod transfer;

use block_history::*;
use clock::*;
use ingest::*;
use network::*;
use peers::*;
use projection::*;
use receipt::*;
use transfer::*;

pub(crate) use block_history::{
    GapSettlement, TouchedRange, observe_gap_settlement, settle_history_gaps_after,
    settle_touched_history_gaps,
};
pub(crate) use boundary::{router, router_v2};
pub(crate) use projection::component_storage_key;
