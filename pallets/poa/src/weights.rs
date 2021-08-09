// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Canyon.
//
// Copyright (c) 2021 Canyon Labs.
//
// Canyon is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published
// by the Free Software Foundation, either version 3 of the License,
// or (at your option) any later version.
//
// Canyon is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with Canyon. If not, see <http://www.gnu.org/licenses/>.

//! Autogenerated weights for pallet_poa
//!
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 4.0.0-dev
//! DATE: 2021-08-05, STEPS: `50`, REPEAT: 20, LOW RANGE: `[]`, HIGH RANGE: `[]`
//! EXECUTION: Some(Wasm), WASM-EXECUTION: Compiled, CHAIN: Some("dev"), DB CACHE: 128

// Executed Command:
// ./target/release/canyon
// benchmark
// --chain
// dev
// --execution=wasm
// --wasm-execution=compiled
// --pallet
// pallet_poa
// --extrinsic
// *
// --steps=50
// --heap-pages=4096
// --repeat
// 20
// --template=./scripts/pallet-weights-template.hbs
// --output=./pallets/poa/src/weights.rs

#![allow(clippy::all)]
#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{
    traits::Get,
    weights::{constants::RocksDbWeight, Weight},
};
use sp_std::marker::PhantomData;

/// Weight functions needed for pallet_poa.
pub trait WeightInfo {
    fn deposit() -> Weight;
    fn set_config() -> Weight;
}

/// Weights for pallet_poa using the Substrate node and recommended hardware.
pub struct SubstrateWeight<T>(PhantomData<T>);
impl<T: frame_system::Config> WeightInfo for SubstrateWeight<T> {
    // Storage: Poa HistoryDepth (r:1 w:1)
    // Storage: Poa PoaConfig (r:1 w:0)
    // Storage: System Digest (r:1 w:1)
    // Storage: Authorship Author (r:1 w:0)
    fn deposit() -> Weight {
        (563_445_000 as Weight)
            .saturating_add(T::DbWeight::get().reads(4 as Weight))
            .saturating_add(T::DbWeight::get().writes(2 as Weight))
    }
    // Storage: Poa PoaConfig (r:0 w:1)
    fn set_config() -> Weight {
        (16_173_000 as Weight).saturating_add(T::DbWeight::get().writes(1 as Weight))
    }
}

// For backwards compatibility and tests
impl WeightInfo for () {
    // Storage: Poa HistoryDepth (r:1 w:1)
    // Storage: Poa PoaConfig (r:1 w:0)
    // Storage: System Digest (r:1 w:1)
    // Storage: Authorship Author (r:1 w:0)
    fn deposit() -> Weight {
        (563_445_000 as Weight)
            .saturating_add(RocksDbWeight::get().reads(4 as Weight))
            .saturating_add(RocksDbWeight::get().writes(2 as Weight))
    }
    // Storage: Poa PoaConfig (r:0 w:1)
    fn set_config() -> Weight {
        (16_173_000 as Weight).saturating_add(RocksDbWeight::get().writes(1 as Weight))
    }
}