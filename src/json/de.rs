use std::{error, fmt};

use serde::de::{self, Deserializer, Deserialize, DeserializeOwned, DeserializeSeed, Visitor,
                SeqAccess, MapAccess, EnumAccess, VariantAccess, IntoDeserializer};
use strtod::strtod;
use base64;
use encode_unicode::{Utf8Char, Utf16Char, U16UtfExt};

/// Everything that can go wrong during deserialization.
#[derive(PartialEq, Eq, Debug, Clone)]
pub enum DecodeJsonError {
    /// Needed more data but got EOF instead.
    UnexpectedEndOfInput,
    /// A generic syntax error. Any valid json would have been ok, but alas...
    Syntax,
    /// A number is valid json but it evaluates to -0 or an infinity
    InvalidNumber,
    /// The content of a string is not utf8, uses wrong escape sequences, etc.
    InvalidStringContent,
    /// An object has multiple entries with the equal keys.
    DuplicateKey,
    /// The input contained valid json followed by at least one non-whitespace byte.
    TrailingCharacters,
    /// Attempted to parse a number as an `i8` that was out of bounds.
    OutOfBoundsI8,
    /// Attempted to parse a number as an `i16` that was out of bounds.
    OutOfBoundsI16,
    /// Attempted to parse a number as an `i32` that was out of bounds.
    OutOfBoundsI32,
    /// Attempted to parse a number as an `i64` that was less than -2^53 or greater than 2^53.
    OutOfBoundsI64,
    /// Attempted to parse a number as an `u8` that was out of bounds.
    OutOfBoundsU8,
    /// Attempted to parse a number as an `u16` that was out of bounds.
    OutOfBoundsU16,
    /// Attempted to parse a number as an `u32` that was out of bounds.
    OutOfBoundsU32,
    /// Attempted to parse a number as an `u64` that was greater than 2^53.
    OutOfBoundsU64,
    /// Chars are represented as strings that contain one unicode scalar value.
    NotAChar,
    /// Attempted to read a string as base64-encoded bytes, but the string was not valid base64.
    Base64(base64::DecodeError),
    ExpectedBool,
    ExpectedNumber,
    ExpectedString,
    ExpectedNull,
    ExpectedArray,
    ExpectedObject,
    ExpectedEnum,
    /// Custom, stringly-typed error.
    Message(String),
}

impl fmt::Display for DecodeJsonError {
    fn fmt(&self, f: &mut fmt::Formatter) -> std::result::Result<(), fmt::Error> {
        fmt::Debug::fmt(self, f)
    }
}

impl error::Error for DecodeJsonError {}

impl de::Error for DecodeJsonError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        DecodeJsonError::Message(msg.to_string())
    }
}

/// A structure that deserializes json encoded legacy message values.
pub struct JsonDeserializer<'de> {
    input: &'de [u8],
}

impl<'de> JsonDeserializer<'de> {
    /// Check whether there are no non-whitespace tokens up until the end of the input.
    pub fn end(&mut self) -> Result<(), DecodeJsonError> {
        match self.peek_ws() {
            Ok(_) => Err(DecodeJsonError::TrailingCharacters),
            Err(DecodeJsonError::UnexpectedEndOfInput) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// Try to parse data from the input. Validates that there are no trailing non-whitespace bytes.
pub fn from_slice<'de, T>(input: &'de [u8]) -> Result<T, DecodeJsonError>
    where T: DeserializeOwned
{
    let mut de = JsonDeserializer::from_slice(input);
    match Deserialize::deserialize(&mut de) {
        Ok(t) => de.end().map(|_| t),
        Err(e) => Err(e),
    }
}

/// Try to parse data from the input, returning the remaining input when done.
pub fn from_slice_partial<'de, T>(input: &'de [u8]) -> Result<(T, &'de [u8]), DecodeJsonError>
    where T: DeserializeOwned
{
    let mut de = JsonDeserializer::from_slice(input);
    match Deserialize::deserialize(&mut de) {
        Ok(t) => Ok((t, de.input)),
        Err(e) => Err(e),
    }
}

fn is_ws(byte: u8) -> bool {
    byte == 0x09 || byte == 0x0A || byte == 0x0D || byte == 0x20
}

fn not_is_ws(byte: u8) -> bool {
    !is_ws(byte)
}

fn is_digit(byte: u8) -> bool {
    byte.is_ascii_digit()
}

impl<'de> JsonDeserializer<'de> {
    /// Creates a `Deserializer` from a `&[u8]`.
    pub fn from_slice(input: &'de [u8]) -> Self {
        JsonDeserializer { input }
    }

    // Returns the next byte without consuming it.
    fn peek(&self) -> Result<u8, DecodeJsonError> {
        match self.input.first() {
            Some(byte) => Ok(*byte),
            None => Err(DecodeJsonError::UnexpectedEndOfInput),
        }
    }

    // Returns the next byte without consuming it, or signals end of input as `None`.
    fn peek_or_end(&self) -> Option<u8> {
        self.input.first().map(|b| *b)
    }

    // Unsafely advance the input slice by 1 byte, to be used only after peeking.
    unsafe fn advance(&mut self) {
        self.input = std::slice::from_raw_parts(self.input.as_ptr().offset(1),
                                                self.input.len() - 1);
    }

    // Unsafely advance the input slice by some bytes.
    unsafe fn advance_by(&mut self, offset: isize) {
        self.input = std::slice::from_raw_parts(self.input.as_ptr().offset(offset),
                                                self.input.len() - (offset as usize));
    }

    // Consumes the next byte and returns it.
    fn next(&mut self) -> Result<u8, DecodeJsonError> {
        match self.input.split_first() {
            Some((head, tail)) => {
                self.input = tail;
                Ok(*head)
            }
            None => Err(DecodeJsonError::UnexpectedEndOfInput),
        }
    }

    // Skips values while the predicate returns true, returns the first non-true value, consuming
    // it as well.
    fn consume_including(&mut self, pred: fn(u8) -> bool) -> Result<u8, DecodeJsonError> {
        loop {
            let next = self.next()?;
            if pred(next) {
                return Ok(next);
            }
        }
    }

    // Consumes as much whitespace as possible, then consumes the next non-whitespace byte.
    fn next_ws(&mut self) -> Result<u8, DecodeJsonError> {
        self.consume_including(not_is_ws)
    }

    fn expect_ws_err(&mut self, exp: u8, err: DecodeJsonError) -> Result<(), DecodeJsonError> {
        if self.next_ws()? == exp {
            Ok(())
        } else {
            Err(err)
        }
    }

    // Skips values while the predicate returns true, returns the first non-true value but does
    // not consume it.
    fn consume_until(&mut self, pred: fn(u8) -> bool) -> Result<u8, DecodeJsonError> {
        loop {
            let peeked = self.peek()?;
            if pred(peeked) {
                unsafe { self.advance() };
            } else {
                return Ok(peeked);
            }
        }
    }

    // Skips values while the predicate returns true.
    fn advance_while(&mut self, pred: fn(u8) -> bool) -> () {
        loop {
            match self.peek_or_end() {
                None => return,
                Some(peeked) => {
                    if pred(peeked) {
                        unsafe { self.advance() };
                    } else {
                        return;
                    }
                }
            }
        }
    }

    // Consumes as much whitespace as possible, then peeks at the next non-whitespace byte.
    fn peek_ws(&mut self) -> Result<u8, DecodeJsonError> {
        self.consume_until(is_ws)
    }

    // Consumes the expected byt, gives the given error if it is something else
    fn expect_err(&mut self, expected: u8, err: DecodeJsonError) -> Result<(), DecodeJsonError> {
        if self.next()? == expected {
            Ok(())
        } else {
            Err(err)
        }
    }

    // Same as expect, but using a predicate.
    fn expect_pred(&mut self, pred: fn(u8) -> bool) -> Result<(), DecodeJsonError> {
        if pred(self.next()?) {
            Ok(())
        } else {
            Err(DecodeJsonError::Syntax)
        }
    }

    fn parse_bool(&mut self) -> Result<bool, DecodeJsonError> {
        if self.input.starts_with(b"true") {
            self.input = &self.input[4..];
            return Ok(true);
        } else if self.input.starts_with(b"false") {
            self.input = &self.input[5..];
            return Ok(false);
        } else {
            Err(DecodeJsonError::ExpectedBool)
        }
    }

    fn parse_number(&mut self) -> Result<f64, DecodeJsonError> {
        let original_input = self.input;

        // trailing `-`
        match self.peek() {
            Ok(0x2D) => unsafe { self.advance() },
            Ok(_) => {}
            Err(DecodeJsonError::UnexpectedEndOfInput) => {
                return Err(DecodeJsonError::ExpectedNumber)
            }
            Err(e) => return Err(e),
        }

        let next = self.next()?;
        match next {
            // first digit `0` must be followed by `.`
            0x30 => {}
            // first digit nonzero, may be followed by more digits until the `.`
            0x31...0x39 => self.advance_while(is_digit),
            _ => return Err(DecodeJsonError::ExpectedNumber),
        }

        // `.`, followed by many1 digits
        if let Some(0x2E) = self.peek_or_end() {
            unsafe {
                self.advance();
            }
            self.expect_pred(is_digit)?;
            self.advance_while(is_digit);
        }

        // `e` or `E`, followed by an optional sign and many1 digits
        match self.peek_or_end() {
            Some(0x45) | Some(0x65) => {
                unsafe {
                    self.advance();
                }

                // optional `+` or `-`
                if self.peek()? == 0x2B || self.peek()? == 0x2D {
                    unsafe {
                        self.advance();
                    }
                }

                // many1 digits
                self.expect_pred(is_digit)?;
                self.advance_while(is_digit);
            }
            _ => {}
        }

        // done parsing the number, convert it to a rust value
        match strtod(unsafe {
                         std::str::from_utf8_unchecked(&original_input[..(original_input.len() -
                                                           self.input.len())])
                     }) {
            Some(parsed) => Ok(parsed),
            None => Err(DecodeJsonError::InvalidNumber),
        }
    }

    // Return a slice beginning and ending with 0x22 (`"`)
    fn parse_naive_string(&mut self) -> Result<&'de [u8], DecodeJsonError> {
        self.expect_err(0x22, DecodeJsonError::ExpectedString)?;
        let start = self.input;

        while self.next()? != 0x22 {
            // noop
        }

        Ok(&start[..(start.len() - (self.input.len() + 1))])
    }

    fn parse_string(&mut self) -> Result<String, DecodeJsonError> {
        self.expect_err(0x22, DecodeJsonError::ExpectedString)?;

        let mut decoded = String::new();

        loop {
            match self.peek()? {
                // terminating `"`, return the decoded string
                0x22 => {
                    unsafe {
                        self.advance();
                    }
                    return Ok(decoded);
                }

                // `\` introduces an escape sequence
                0x5C => {
                    unsafe {
                        self.advance();
                    }

                    match self.next()? {
                        // single character escape sequences
                        0x22 => decoded.push_str("\u{22}"), // `\"`
                        0x5C => decoded.push_str("\u{5C}"), // `\\`
                        0x2F => decoded.push_str("\u{2F}"), // `\/`
                        0x62 => decoded.push_str("\u{08}"), // `\b`
                        0x66 => decoded.push_str("\u{0C}"), // `\f`
                        0x6E => decoded.push_str("\u{0A}"), // `\n`
                        0x72 => decoded.push_str("\u{0D}"), // `\r`
                        0x74 => decoded.push_str("\u{09}"), // `\t`

                        // unicode escape sequences
                        0x75 => {
                            if self.input.len() < 4 {
                                return Err(DecodeJsonError::InvalidStringContent);
                            }

                            match u16::from_str_radix(unsafe {
                                std::str::from_utf8_unchecked(&self.input[..4])
                            }, 16) {
                                Ok(code_point) => {
                                    unsafe {
                                        self.advance_by(4);
                                    }

                                    if code_point.is_utf16_leading_surrogate() {
                                        // the unicode escape was for a leading surrogate, which
                                        // must be followed by another unicode escape which is a
                                        // trailing surrogate
                                        self.expect_err(0x5C, DecodeJsonError::InvalidStringContent)?;
                                        self.expect_err(0x75, DecodeJsonError::InvalidStringContent)?;
                                        if self.input.len() < 4 {
                                            return Err(DecodeJsonError::InvalidStringContent);
                                        }

                                        match u16::from_str_radix(unsafe {
                                            std::str::from_utf8_unchecked(&self.input[..4])
                                        }, 16) {
                                            Ok(code_point2) => {
                                                match Utf16Char::from_tuple((code_point, Some(code_point2))) {
                                                    Ok(c) => decoded.push(c.into()),
                                                    Err(_) => return Err(DecodeJsonError::InvalidStringContent),
                                                }
                                            }
                                            Err(_) => return Err(DecodeJsonError::InvalidStringContent),
                                        }
                                    } else {
                                        match std::char::from_u32(code_point as u32) {
                                            Some(c) => decoded.push(c),
                                            None => return Err(DecodeJsonError::InvalidStringContent),
                                        }
                                    }
                                }
                                Err(_) => return Err(DecodeJsonError::InvalidStringContent),
                            }
                        }

                        // Nothing else may follow an unescaped `\`
                        _ => return Err(DecodeJsonError::InvalidStringContent),
                    }
                }

                // the control code points must be escaped
                0x00...0x1F => return Err(DecodeJsonError::InvalidStringContent),

                // a regular utf8-encoded code point (unless it is malformed)
                _ => {
                    match Utf8Char::from_slice_start(self.input) {
                        Err(_) => return Err(DecodeJsonError::InvalidStringContent),
                        Ok((_, len)) => unsafe {
                            decoded.push_str(std::str::from_utf8_unchecked(&self.input[..len]));
                            self.advance_by(len as isize);
                        },
                    }
                }
            }
        }
    }

    fn parse_null(&mut self) -> Result<(), DecodeJsonError> {
        if self.input.starts_with(b"null") {
            self.input = &self.input[4..];
            return Ok(());
        } else {
            Err(DecodeJsonError::ExpectedNull)
        }
    }
}

impl<'de, 'a> Deserializer<'de> for &'a mut JsonDeserializer<'de> {
    type Error = DecodeJsonError;

    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value, DecodeJsonError>
        where V: Visitor<'de>
    {
        match self.peek_ws()? {
            0x6E => {
                if self.input[1..].starts_with(b"ull") {
                    self.input = &self.input[4..];
                    visitor.visit_unit()
                } else {
                    Err(DecodeJsonError::Syntax)
                }
            }
            0x66 => {
                if self.input[1..].starts_with(b"alse") {
                    self.input = &self.input[5..];
                    visitor.visit_bool(false)
                } else {
                    Err(DecodeJsonError::Syntax)
                }
            }
            0x74 => {
                if self.input[1..].starts_with(b"rue") {
                    self.input = &self.input[4..];
                    visitor.visit_bool(true)
                } else {
                    Err(DecodeJsonError::Syntax)
                }
            }
            0x22 => self.deserialize_str(visitor),
            0x5B => self.deserialize_seq(visitor),
            0x7B => self.deserialize_map(visitor),
            0x2D | 0x30...0x39 => self.deserialize_f64(visitor),
            _ => Err(DecodeJsonError::Syntax),
        }
    }

    fn deserialize_bool<V>(self, visitor: V) -> Result<V::Value, DecodeJsonError>
        where V: Visitor<'de>
    {
        visitor.visit_bool(self.parse_bool()?)
    }

    fn deserialize_i8<V>(self, visitor: V) -> Result<V::Value, DecodeJsonError>
        where V: Visitor<'de>
    {
        let f = self.parse_number()?;
        if f < std::i8::MIN as f64 || f > std::i8::MAX as f64 {
            Err(DecodeJsonError::OutOfBoundsI8)
        } else {
            visitor.visit_i8(f as i8)
        }
    }

    fn deserialize_i16<V>(self, visitor: V) -> Result<V::Value, DecodeJsonError>
        where V: Visitor<'de>
    {
        let f = self.parse_number()?;
        if f < std::i16::MIN as f64 || f > std::i16::MAX as f64 {
            Err(DecodeJsonError::OutOfBoundsI16)
        } else {
            visitor.visit_i16(f as i16)
        }
    }

    fn deserialize_i32<V>(self, visitor: V) -> Result<V::Value, DecodeJsonError>
        where V: Visitor<'de>
    {
        let f = self.parse_number()?;
        if f < std::i32::MIN as f64 || f > std::i32::MAX as f64 {
            Err(DecodeJsonError::OutOfBoundsI32)
        } else {
            visitor.visit_i32(f as i32)
        }
    }

    fn deserialize_i64<V>(self, visitor: V) -> Result<V::Value, DecodeJsonError>
        where V: Visitor<'de>
    {
        let f = self.parse_number()?;
        if f < -9007199254740992.0f64 || f > 9007199254740992.0f64 {
            Err(DecodeJsonError::OutOfBoundsI64)
        } else {
            visitor.visit_i64(f as i64)
        }
    }

    fn deserialize_u8<V>(self, visitor: V) -> Result<V::Value, DecodeJsonError>
        where V: Visitor<'de>
    {
        let f = self.parse_number()?;
        if f > std::u8::MAX as f64 {
            Err(DecodeJsonError::OutOfBoundsU8)
        } else {
            visitor.visit_u8(f as u8)
        }
    }

    fn deserialize_u16<V>(self, visitor: V) -> Result<V::Value, DecodeJsonError>
        where V: Visitor<'de>
    {
        let f = self.parse_number()?;
        if f > std::u16::MAX as f64 {
            Err(DecodeJsonError::OutOfBoundsU16)
        } else {
            visitor.visit_u16(f as u16)
        }
    }

    fn deserialize_u32<V>(self, visitor: V) -> Result<V::Value, DecodeJsonError>
        where V: Visitor<'de>
    {
        let f = self.parse_number()?;
        if f > std::u32::MAX as f64 {
            Err(DecodeJsonError::OutOfBoundsU32)
        } else {
            visitor.visit_u32(f as u32)
        }
    }

    fn deserialize_u64<V>(self, visitor: V) -> Result<V::Value, DecodeJsonError>
        where V: Visitor<'de>
    {
        let f = self.parse_number()?;
        if f > 9007199254740992.0f64 {
            Err(DecodeJsonError::OutOfBoundsU64)
        } else {
            visitor.visit_u64(f as u64)
        }
    }

    fn deserialize_f32<V>(self, visitor: V) -> Result<V::Value, DecodeJsonError>
        where V: Visitor<'de>
    {
        visitor.visit_f32(self.parse_number()? as f32)
    }

    fn deserialize_f64<V>(self, visitor: V) -> Result<V::Value, DecodeJsonError>
        where V: Visitor<'de>
    {
        visitor.visit_f64(self.parse_number()?)
    }

    fn deserialize_char<V>(self, visitor: V) -> Result<V::Value, DecodeJsonError>
        where V: Visitor<'de>
    {
        let s = self.parse_string()?;
        let mut chars = s.chars();

        match chars.next() {
            None => return Err(DecodeJsonError::NotAChar),
            Some(c) => {
                match chars.next() {
                    None => return visitor.visit_char(c),
                    Some(_) => return Err(DecodeJsonError::NotAChar),
                }
            }
        }
    }

    fn deserialize_str<V>(self, visitor: V) -> Result<V::Value, DecodeJsonError>
        where V: Visitor<'de>
    {
        // We can't reference json strings directly since they contain escape sequences.
        // For the conversion, we need to allocate an owned buffer, so always do owned
        // deserialization.
        self.deserialize_string(visitor)
    }

    fn deserialize_string<V>(self, visitor: V) -> Result<V::Value, DecodeJsonError>
        where V: Visitor<'de>
    {
        visitor.visit_string(self.parse_string()?)
    }

    fn deserialize_bytes<V>(self, visitor: V) -> Result<V::Value, DecodeJsonError>
        where V: Visitor<'de>
    {
        // We can't reference bytes directly since they are stored as base64 strings.
        // For the conversion, we need to allocate an owned buffer, so always do owned
        // deserialization.
        self.deserialize_byte_buf(visitor)
    }

    fn deserialize_byte_buf<V>(self, visitor: V) -> Result<V::Value, DecodeJsonError>
        where V: Visitor<'de>
    {
        match base64::decode(self.parse_naive_string()?) {
            Ok(buf) => visitor.visit_byte_buf(buf),
            Err(e) => Err(DecodeJsonError::Base64(e)),
        }
    }

    fn deserialize_option<V>(self, visitor: V) -> Result<V::Value, DecodeJsonError>
        where V: Visitor<'de>
    {
        if self.input.starts_with(b"null") {
            self.input = &self.input[4..];
            visitor.visit_none()
        } else {
            visitor.visit_some(self)
        }
    }

    fn deserialize_unit<V>(self, visitor: V) -> Result<V::Value, DecodeJsonError>
        where V: Visitor<'de>
    {
        self.parse_null()?;
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V>(self,
                                  _name: &'static str,
                                  visitor: V)
                                  -> Result<V::Value, DecodeJsonError>
        where V: Visitor<'de>
    {
        self.deserialize_unit(visitor)
    }

    fn deserialize_newtype_struct<V>(self,
                                     _name: &'static str,
                                     visitor: V)
                                     -> Result<V::Value, DecodeJsonError>
        where V: Visitor<'de>
    {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V>(mut self, visitor: V) -> Result<V::Value, DecodeJsonError>
        where V: Visitor<'de>
    {
        self.expect_err(0x5B, DecodeJsonError::ExpectedArray)?;
        let value = visitor.visit_seq(CollectionAccessor::new(&mut self))?;
        self.expect_ws_err(0x5D, DecodeJsonError::Syntax)?;
        Ok(value)
    }

    fn deserialize_tuple<V>(self, _len: usize, visitor: V) -> Result<V::Value, DecodeJsonError>
        where V: Visitor<'de>
    {
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V>(self,
                                   _name: &'static str,
                                   _len: usize,
                                   visitor: V)
                                   -> Result<V::Value, DecodeJsonError>
        where V: Visitor<'de>
    {
        self.deserialize_seq(visitor)
    }

    fn deserialize_map<V>(mut self, visitor: V) -> Result<V::Value, DecodeJsonError>
        where V: Visitor<'de>
    {
        self.expect_err(0x7B, DecodeJsonError::ExpectedObject)?;
        let value = visitor.visit_map(CollectionAccessor::new(&mut self))?;
        self.expect_ws_err(0x7D, DecodeJsonError::Syntax)?;
        Ok(value)
    }

    fn deserialize_struct<V>(self,
                             _name: &'static str,
                             _fields: &'static [&'static str],
                             visitor: V)
                             -> Result<V::Value, DecodeJsonError>
        where V: Visitor<'de>
    {
        self.deserialize_map(visitor)
    }

    fn deserialize_enum<V>(self,
                           _name: &'static str,
                           _variants: &'static [&'static str],
                           visitor: V)
                           -> Result<V::Value, DecodeJsonError>
        where V: Visitor<'de>
    {
        if self.peek()? == 0x22 {
            // Visit a unit variant.
            visitor.visit_enum(self.parse_string()?.into_deserializer())
        } else if self.next()? == 0x7B {
            // Visit a newtype variant, tuple variant, or struct variant.
            let value = visitor.visit_enum(Enum::new(self))?;
            self.expect_ws_err(0x7D, DecodeJsonError::Syntax)?;
            Ok(value)
        } else {
            Err(DecodeJsonError::ExpectedEnum)
        }
    }

    fn deserialize_identifier<V>(self, visitor: V) -> Result<V::Value, DecodeJsonError>
        where V: Visitor<'de>
    {
        self.deserialize_str(visitor)
    }

    fn deserialize_ignored_any<V>(self, visitor: V) -> Result<V::Value, DecodeJsonError>
        where V: Visitor<'de>
    {
        self.deserialize_any(visitor)
    }
}

struct CollectionAccessor<'de, 'a> {
    des: &'a mut JsonDeserializer<'de>,
    first: bool,
}

impl<'de, 'a> CollectionAccessor<'de, 'a> {
    fn new(des: &'a mut JsonDeserializer<'de>) -> CollectionAccessor<'de, 'a> {
        CollectionAccessor { des, first: true }
    }
}

impl<'de, 'a> SeqAccess<'de> for CollectionAccessor<'de, 'a> {
    type Error = DecodeJsonError;

    fn next_element_seed<T>(&mut self, seed: T) -> Result<Option<T::Value>, DecodeJsonError>
        where T: DeserializeSeed<'de>
    {
        // Array ends at `]`
        if let 0x5D = self.des.peek_ws()? {
            return Ok(None);
        }

        // expect `,` before every item except the first
        if self.first {
            self.first = false;
        } else {
            self.des.expect_ws_err(0x2C, DecodeJsonError::Syntax)?;
        }

        self.des.consume_until(is_ws)?;

        seed.deserialize(&mut *self.des).map(Some)
    }
}

impl<'de, 'a> MapAccess<'de> for CollectionAccessor<'de, 'a> {
    type Error = DecodeJsonError;

    fn next_key_seed<K>(&mut self, seed: K) -> Result<Option<K::Value>, DecodeJsonError>
        where K: DeserializeSeed<'de>
    {
        // Object ends at `}`
        if let 0x7D = self.des.peek_ws()? {
            return Ok(None);
        }

        // expect `,` before every item except the first
        if self.first {
            self.first = false;
        } else {
            self.des.expect_ws_err(0x2C, DecodeJsonError::Syntax)?;
        }

        self.des.consume_until(is_ws)?;
        seed.deserialize(&mut *self.des).map(Some)
    }

    fn next_value_seed<V>(&mut self, seed: V) -> Result<V::Value, DecodeJsonError>
        where V: DeserializeSeed<'de>
    {
        self.des.expect_ws_err(0x3A, DecodeJsonError::Syntax)?; // `:`

        self.des.consume_until(is_ws)?;
        seed.deserialize(&mut *self.des)
    }
}

struct Enum<'a, 'de: 'a> {
    des: &'a mut JsonDeserializer<'de>,
}

impl<'a, 'de> Enum<'a, 'de> {
    fn new(des: &'a mut JsonDeserializer<'de>) -> Self {
        Enum { des }
    }
}

impl<'de, 'a> EnumAccess<'de> for Enum<'a, 'de> {
    type Error = DecodeJsonError;
    type Variant = Self;

    fn variant_seed<V>(self, seed: V) -> Result<(V::Value, Self::Variant), DecodeJsonError>
        where V: DeserializeSeed<'de>
    {
        self.des.consume_until(is_ws)?;
        let val = seed.deserialize(&mut *self.des)?;
        self.des.expect_ws_err(0x3A, DecodeJsonError::Syntax)?; // `:`

        self.des.consume_until(is_ws)?;
        Ok((val, self))
    }
}

impl<'de, 'a> VariantAccess<'de> for Enum<'a, 'de> {
    type Error = DecodeJsonError;

    fn unit_variant(self) -> Result<(), DecodeJsonError> {
        Err(DecodeJsonError::ExpectedString)
    }

    fn newtype_variant_seed<T>(self, seed: T) -> Result<T::Value, DecodeJsonError>
        where T: DeserializeSeed<'de>
    {
        seed.deserialize(self.des)
    }

    fn tuple_variant<V>(self, _len: usize, visitor: V) -> Result<V::Value, DecodeJsonError>
        where V: Visitor<'de>
    {
        de::Deserializer::deserialize_seq(self.des, visitor)
    }

    // Struct variants are represented in JSON as `{ NAME: { K: V, ... } }` so
    // deserialize the inner map here.
    fn struct_variant<V>(self,
                         _fields: &'static [&'static str],
                         visitor: V)
                         -> Result<V::Value, DecodeJsonError>
        where V: Visitor<'de>
    {
        de::Deserializer::deserialize_map(self.des, visitor)
    }
}

// #[cfg(test)]
// mod tests {
//     use super::super::{Value, from_slice, to_vec};
//
//     fn check(input: &[u8]) {
//         let val = from_slice::<Value>(input).unwrap();
//         println!("{:?}", val);
//         let enc = to_vec(&val, true);
//         let enc_string = std::str::from_utf8(&enc).unwrap().to_string();
//         println!("{}\n{:?}\n{:x?}", enc_string, enc_string, enc);
//         let redecoded = from_slice::<Value>(&enc[..]).unwrap();
//         assert_eq!(val, redecoded);
//     }
//
//     #[test]
//     fn regression() {
//         // check(&[34, 110, 193, 146, 34][..]);
//         // check(br##"[[][[[][][]][]]]"##);
//         // check(b"888e-39919999992999999999999999999999999999999999993");
//         // check(br##"11111111111111111111111111111111111111111111111111111111111111111111111111e-323"##);
//         // check(br##"8391.8999999999999999999928e-328e-8"##);
//         // check(br##"839999999999999999999928e-338e-9"##);
//     }
// }
