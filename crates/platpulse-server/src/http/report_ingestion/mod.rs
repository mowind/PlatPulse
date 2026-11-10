//! Transactional AgentReport ingestion for the first Inventory vertical slice.
//!
//! The stages of one Report live in their own modules: [`boundary`] admits the
//! request, [`admission`] decides the whole-Report verdict, [`ingest`] runs the transaction, [`receipt`] builds the receipt or
//! rejection, [`clock`] places observations on the Server clock, [`projection`],
//! [`peers`] and [`crate::block_history`] write what the Report observed, [`network`]
//! recomputes the Observed Network Head, and [`transfer`] resolves Node
//! transfers between Agents.

mod admission;
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

use admission::*;
use clock::*;
use ingest::*;
use network::*;
use peers::*;
use projection::*;
use receipt::*;
use transfer::*;

pub(crate) use boundary::{router, router_v2};
pub(crate) use projection::component_storage_key;
