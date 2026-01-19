pub mod ast;
pub mod runes;
pub mod utils;

extern crate self as parser;

#[path = "main.rs"]
mod parser_main;

pub use parser_main::parser as native_parser;
