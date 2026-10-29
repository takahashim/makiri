//! `Makiri::XML::SAX`'s native entry: the parse the Ruby `Parser`
//! (`lib/makiri/xml/sax.rb`) calls with its handler.

use crate::bridge::rb::{prelude::*, Error, Ruby, Value};
use crate::function;

/// `Makiri::XML::SAX._parse(handler, source, attribute_class)`.
fn s_parse(ruby: &Ruby, handler: Value, source: Value, attribute: Value) -> Result<(), Error> {
    crate::bridge::xml_sax::parse(ruby, handler, source, attribute)
}

/// From `Init_makiri`, with `Makiri::XML` defined.
pub fn init_xml_sax(ruby: &Ruby) -> Result<(), Error> {
    let sax = crate::init::MOD_XML.defined(ruby)?.define_module("SAX")?;
    sax.define_singleton_method("_parse", function!(s_parse, 3))?;
    Ok(())
}
