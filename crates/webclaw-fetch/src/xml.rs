use quick_xml::escape::unescape;
use quick_xml::events::attributes::Attribute;
use quick_xml::events::{BytesRef, BytesText};
use quick_xml::{Decoder, XmlVersion};

pub(crate) fn decode_text(event: &BytesText<'_>) -> Option<String> {
    event.xml10_content().ok().map(|text| text.into_owned())
}

pub(crate) fn decode_reference(event: &BytesRef<'_>) -> Option<String> {
    let reference = event.decode().ok()?;
    unescape(&format!("&{reference};"))
        .ok()
        .map(|text| text.into_owned())
}

pub(crate) fn decode_attribute(attribute: &Attribute<'_>, decoder: Decoder) -> Option<String> {
    attribute
        .decoded_and_normalized_value(XmlVersion::Implicit1_0, decoder)
        .ok()
        .map(|value| value.into_owned())
}

#[cfg(test)]
mod tests {
    use quick_xml::Reader;
    use quick_xml::events::Event;

    use super::*;

    #[test]
    fn decodes_text_and_attributes_with_entities() {
        let mut reader = Reader::from_str(r#"<item name="A &amp; B">C &lt; D</item>"#);
        let mut attribute = None;
        let mut text = String::new();

        loop {
            match reader.read_event().unwrap() {
                Event::Start(start) => {
                    attribute = start
                        .attributes()
                        .next()
                        .and_then(Result::ok)
                        .and_then(|value| decode_attribute(&value, reader.decoder()));
                }
                Event::Text(value) => text.push_str(&decode_text(&value).unwrap()),
                Event::GeneralRef(value) => {
                    text.push_str(&decode_reference(&value).unwrap());
                }
                Event::Eof => break,
                _ => {}
            }
        }

        assert_eq!(attribute.as_deref(), Some("A & B"));
        assert_eq!(text, "C < D");
    }
}
