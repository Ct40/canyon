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

//! A `CodeExecutor` specialization which uses natively compiled runtime when the wasm to be
//! executed is equivalent to the natively compiled code.

use sc_executor::native_executor_instance;
pub use sc_executor::NativeExecutor;

// Declare an instance of the native executor named `Executor`. Include the wasm binary as the
// equivalent wasm code.
native_executor_instance!(
    pub Executor,
    canyon_runtime::api::dispatch,
    canyon_runtime::native_version,
    frame_benchmarking::benchmarking::HostFunctions,
);