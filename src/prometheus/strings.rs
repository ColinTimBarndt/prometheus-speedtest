use core::fmt;
use std::{
    borrow::Borrow,
    fmt::{Display, Write as _},
    ops::{Deref, Index},
    slice::SliceIndex,
    str::Chars,
};

use thiserror::Error;

#[derive(Debug, Clone)]
pub struct EscapePrometheus<'s> {
    inner: Chars<'s>,
    esc_char: Option<char>,
}

impl Iterator for EscapePrometheus<'_> {
    type Item = char;
    fn next(&mut self) -> Option<Self::Item> {
        if let Some(ch) = self.esc_char.take() {
            return Some(ch);
        }
        let ch = self.inner.next()?;
        self.esc_char = match ch {
            '\\' => Some('\\'),
            '"' => Some('"'),
            '\n' => Some('n'),
            _ => None,
        };
        Some(if self.esc_char.is_some() { '\\' } else { ch })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl Display for EscapePrometheus<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for ch in self.clone() {
            f.write_char(ch)?;
        }
        Ok(())
    }
}

pub fn escape_prometheus_str(str: &str) -> EscapePrometheus {
    EscapePrometheus {
        inner: str.chars(),
        esc_char: None,
    }
}

#[derive(Debug, Error, Clone)]
#[error("illegal characters in prometheus name")]
pub struct InvalidPrometheusNameError;

pub fn is_valid_prometheus_name(name: &str) -> bool {
    name.chars().all(|ch| matches!(ch, 'a'..='z' | '_'))
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
/// Prometheus identifier
pub struct PName(str);

impl PName {
    pub const QUANTILE: &'static Self = unsafe { Self::new_unchecked("quantile") };
    pub const LE: &'static Self = unsafe { Self::new_unchecked("le") };
    pub const SUFFIX_BUCKET: &'static Self = unsafe { Self::new_unchecked("_bucket") };
    pub const SUFFIX_SUM: &'static Self = unsafe { Self::new_unchecked("_sum") };
    pub const SUFFIX_COUNT: &'static Self = unsafe { Self::new_unchecked("_count") };

    pub fn new(name: &str) -> Result<&Self, InvalidPrometheusNameError> {
        if is_valid_prometheus_name(name) {
            Ok(unsafe { PName::new_unchecked(name) })
        } else {
            Err(InvalidPrometheusNameError)
        }
    }

    /// # Safety
    /// The given string must only contain the characters `[a-z_]`.
    pub const unsafe fn new_unchecked(name: &str) -> &Self {
        &*(name as *const str as *const PName)
    }
}

impl Default for &PName {
    fn default() -> Self {
        unsafe { PName::new_unchecked("") }
    }
}

impl AsRef<str> for PName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Deref for PName {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl fmt::Debug for PName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        <str as fmt::Debug>::fmt(self.as_ref(), f)
    }
}

impl fmt::Display for PName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        <str as fmt::Display>::fmt(self.as_ref(), f)
    }
}

impl ToOwned for PName {
    type Owned = PNameBuf;

    fn to_owned(&self) -> Self::Owned {
        PNameBuf(self.0.to_owned())
    }
}

impl<B: SliceIndex<str, Output = str>> Index<B> for PName {
    type Output = PName;

    fn index(&self, index: B) -> &Self::Output {
        unsafe { PName::new_unchecked(&self.0[index]) }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct PNameBuf(String);

impl PNameBuf {
    pub const fn new() -> Self {
        Self(String::new())
    }

    pub fn push_name(&mut self, name: &PName) {
        self.0.push_str(name.as_ref())
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn truncate(&mut self, new_len: usize) {
        self.0.truncate(new_len);
    }
}

impl AsRef<PName> for PNameBuf {
    fn as_ref(&self) -> &PName {
        unsafe { PName::new_unchecked(&self.0) }
    }
}

impl Deref for PNameBuf {
    type Target = PName;

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl Borrow<PName> for PNameBuf {
    fn borrow(&self) -> &PName {
        self.as_ref()
    }
}

#[derive(Debug, Default, Clone)]
pub struct PNameBuilder {
    buf: PNameBuf,
    waypoints: Vec<usize>,
}

impl PNameBuilder {
    #[inline]
    pub const fn new() -> Self {
        Self {
            buf: PNameBuf::new(),
            waypoints: Vec::new(),
        }
    }

    #[inline]
    pub fn push(&mut self, name: &PName) {
        self.waypoints.push(self.buf.len());
        self.buf.push_name(name)
    }

    #[inline]
    pub fn pop(&mut self) -> bool {
        if let Some(len) = self.waypoints.pop() {
            self.buf.truncate(len);
            true
        } else {
            false
        }
    }

    #[inline]
    pub fn clear(&mut self) {
        self.buf.clear();
        self.waypoints.clear();
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
}

impl AsRef<PName> for PNameBuilder {
    #[inline]
    fn as_ref(&self) -> &PName {
        &self.buf
    }
}

impl Display for PNameBuilder {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        <PName as Display>::fmt(self.as_ref(), f)
    }
}
