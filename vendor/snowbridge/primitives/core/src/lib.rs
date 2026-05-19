// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: 2023 Snowfork <hello@snowfork.com>
//! # Core (trimmed for bridgechain)
//!
//! Original `snowbridge-core` is bonded to XCM and parachain primitives. The
//! ethereum-client pallet only needs a tiny subset of it: a paused/active
//! operating-mode enum and a fixed-size ring-buffer map. Everything else
//! (XCM Location helpers, agent/token IDs, parachain channels, pricing,
//! rewards) is stripped because this chain is a solochain and doesn't
//! participate in XCM.
//!
//! When upstream snowbridge-core changes shape, re-vendor and re-strip; the
//! delta is intentionally small.
#![cfg_attr(not(feature = "std"), no_std)]

pub mod operating_mode;
pub mod ringbuffer;

pub use operating_mode::BasicOperatingMode;
pub use ringbuffer::{RingBufferMap, RingBufferMapImpl};
