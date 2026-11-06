//! Transactional AgentReport ingestion for the first Inventory vertical slice.
//!
//! The stages of one Report live in their own modules: [`boundary`] admits the
//! request, [`ingest`] runs the transaction, [`receipt`] builds the receipt or
//! rejection, [`clock`] places observations on the Server clock, [`projection`],
//! [`peers`] and [`crate::block_history`] write what the Report observed, [`network`]
//! recomputes the Observed Network Head, and [`transfer`] resolves Node
//! transfers between Agents.

#[cfg(test)]
mod block_history_tests;
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

use clock::*;
use ingest::*;
use network::*;
use peers::*;
use projection::*;
use receipt::*;
use transfer::*;

pub(crate) use boundary::{router, router_v2};
pub(crate) use projection::component_storage_key;
