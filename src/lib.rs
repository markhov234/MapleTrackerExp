//! Shared code for the EXP Calc overlay and its command-line tools.
//!
//! The product's hard boundary lives here as much as anywhere: these modules
//! read pixel buffers and do arithmetic. Nothing reads game memory, sends
//! input, or inspects network traffic.

pub mod chat;
pub mod expcurve;
pub mod history;
pub mod notes;
pub mod ocr;
pub mod region;
pub mod session;
pub mod tracker;
pub mod unstretch;
