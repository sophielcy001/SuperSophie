// SPDX-License-Identifier: AGPL-3.0-or-later

//! Self-hosted personal sync service for Zhiye (milestone M6).

mod blobs;
pub mod engine;
pub mod http;
mod store;

pub use engine::{Device, Engine, ServiceError, ServiceResult};
pub use http::{Service, serve};
