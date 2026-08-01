#![allow(
    missing_docs,
    reason = "output variants are self-describing CLI values"
)]

use std::io::{self, Write};
use std::str::FromStr;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

/// Public output representations.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    Text,
    Json,
    Ndjson,
}

impl FromStr for OutputFormat {
    type Err = ();
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "text" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            "ndjson" => Ok(Self::Ndjson),
            _ => Err(()),
        }
    }
}

/// Raw download mode writes no diagnostic bytes.
///
/// # Errors
///
/// Returns the output writer's I/O error.
pub fn render_raw_download(
    bytes: &[u8],
    stdout: &mut impl Write,
    _stderr: &mut impl Write,
) -> io::Result<()> {
    stdout.write_all(bytes)?;
    stdout.flush()
}
