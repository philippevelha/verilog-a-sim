//! Line-oriented netlist parser.

use crate::{Netlist, NetlistError};

/// Parse a netlist deck into a [`Netlist`].
///
/// # Errors
///
/// Returns [`NetlistError::Parse`] on a malformed line, or [`NetlistError::UnknownModel`] if
/// a device names a model that was never declared.
pub fn parse(_deck: &str) -> Result<Netlist, NetlistError> {
    todo!("T6: tokenize device lines, build the node map, collect devices")
}
