extern crate indexmap;
extern crate ryu_ecmascript;
extern crate strtod;
extern crate encode_unicode;

pub mod abstract_;
pub mod json;


/// An iterator that yields the bytes needed to compute the hash of a message.
/// The total number of bytes yielded by this is the length of the message.
pub struct WeirdEncodingIterator<'a>(std::iter::Map<std::str::EncodeUtf16<'a>, fn(u16) -> u8>);

impl<'a> Iterator for WeirdEncodingIterator<'a> {
    type Item = u8;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }
}

/// Create an owned representation of the weird encoding used for hash computation of legacy ssb
/// messages. The length of this is also the value you need for checking maximum message size.
pub fn to_weird_encoding<'a>(s: &'a str) -> WeirdEncodingIterator<'a> {
    WeirdEncodingIterator(s.encode_utf16().map(shiftr8))
}

/// Compute the length of a message. Note that this takes time linear in the length of the message,
/// so you might want to use a `WeirdEncodingIterator` for computing hash and length in one go.
pub fn legacy_length(msg: &str) -> usize {
    to_weird_encoding(msg).count()
}

fn shiftr8(x: u16) -> u8 {
    (x >> 8) as u8
}
