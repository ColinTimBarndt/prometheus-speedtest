use std::{
    collections::HashMap,
    fmt::{self, Debug, Display, Write},
    mem,
    time::SystemTime,
};

mod go_floats;
mod strings;

pub use go_floats::*;
pub use strings::*;
use typed_arena::Arena;

pub struct ExpositionBuilder<'a> {
    alloc: &'a Arena<u8>,
    buffer: String,
    entries: HashMap<&'a PName, MetricGroup<'a>>,
    pub labels: LabelBuilder,
    pub name: PNameBuilder,
}

struct MetricGroup<'a> {
    help: &'a str,
    lines: Vec<&'a str>,
}

impl<'a> ExpositionBuilder<'a> {
    #[inline]
    pub fn new(alloc: &'a Arena<u8>) -> Self {
        Self {
            alloc,
            buffer: String::new(),
            entries: HashMap::new(),
            labels: LabelBuilder::new(),
            name: PNameBuilder::new(),
        }
    }

    #[inline]
    pub fn with_label<R>(
        &mut self,
        name: &PName,
        value: &(impl SerializePrometheusLabelValue + ?Sized),
        closure: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.labels.push(name, value);
        let r = closure(self);
        self.labels.pop();
        r
    }

    #[inline]
    pub fn with_name<R>(&mut self, name: &PName, closure: impl FnOnce(&mut Self) -> R) -> R {
        self.name.push(name);
        let r = closure(self);
        self.name.pop();
        r
    }

    #[inline]
    pub fn add_metric<R>(
        &mut self,
        metric_suffix: &PName,
        metric_type: MetricType,
        help_text: impl PrometheusHelpTextSource,
        closure: impl FnOnce(ExpositionMetricBuilder<'a, '_>) -> R,
    ) -> R {
        self.name.push(metric_suffix);

        let group_name = if let Some((key, _)) = self.entries.get_key_value(self.name.as_ref()) {
            key
        } else {
            let group_name = self.alloc_pname(self.name.as_ref());
            let metric_name = self.name.as_ref();
            self.buffer.clear();
            write!(self.buffer, "# HELP {metric_name} ").unwrap();
            let help_text_start = self.buffer.len();
            help_text.write_help_text(&mut self.buffer);
            // SAFETY: Replaces ASCII char with another ASCII char
            unsafe {
                let raw = &mut self.buffer.as_mut_vec()[help_text_start..];
                for byte in raw {
                    if *byte == b'\n' {
                        *byte = b' ';
                    }
                }
            }
            writeln!(self.buffer, "\n# TYPE {metric_name} {metric_type}").unwrap();
            let group = MetricGroup {
                help: self.alloc.alloc_str(&self.buffer[..]),
                lines: Vec::new(),
            };
            self.entries.insert(group_name, group);
            group_name
        };

        let saved_name = mem::take(&mut self.name);
        // Only store added suffixes
        let r = closure(ExpositionMetricBuilder {
            inner: self,
            group_name,
        });
        self.name = saved_name;
        self.name.pop();
        r
    }

    fn alloc_pname(&self, pname: &PName) -> &'a PName {
        unsafe { PName::new_unchecked(self.alloc.alloc_str(pname.as_ref())) }
    }
}

impl<'s, 'a> Display for ExpositionBuilder<'a>
where
    Self: 's,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut sorted: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, group)| !group.lines.is_empty())
            .map(|(k, v)| (*k, v))
            .collect();
        sorted.sort_unstable_by_key(|(k, _)| *k);
        for (name, group) in sorted {
            f.write_str(group.help)?;
            for line in &group.lines {
                f.write_str(name)?;
                f.write_str(line)?;
            }
        }
        Ok(())
    }
}

pub struct ExpositionMetricBuilder<'a, 'b> {
    inner: &'b mut ExpositionBuilder<'a>,
    group_name: &'a PName,
}

impl ExpositionMetricBuilder<'_, '_> {
    #[inline]
    pub fn add_line(&mut self, data: &(impl SerializeGoFloat + ?Sized), at: Option<SystemTime>) {
        self.inner.buffer.clear();
        // Note that this is only the suffix being pushed, if any
        self.inner.buffer.push_str(self.inner.name.as_ref());
        write!(self.inner.buffer, "{} ", self.inner.labels).unwrap();
        data.serialize_go_float(&mut self.inner.buffer).unwrap();
        if let Some(at) = at {
            write!(
                self.inner.buffer,
                "{}",
                at.duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis()
            )
            .unwrap();
        }
        self.inner.buffer.push('\n');
        self.add_line_entry();
    }

    #[inline]
    pub fn add_line_labeled(
        &mut self,
        label: &PName,
        value: &(impl SerializePrometheusLabelValue + ?Sized),
        data: &(impl SerializeGoFloat + ?Sized),
        at: Option<SystemTime>,
    ) {
        self.inner.buffer.clear();
        // Note that this is only the suffix being pushed, if any
        self.inner.buffer.push_str(self.inner.name.as_ref());
        self.inner.with_label(label, value, |builder| {
            write!(builder.buffer, "{} ", builder.labels).unwrap();
        });
        data.serialize_go_float(&mut self.inner.buffer).unwrap();
        if let Some(at) = at {
            write!(
                self.inner.buffer,
                "{}",
                at.duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis()
            )
            .unwrap();
        }
        self.inner.buffer.push('\n');
        self.add_line_entry();
    }

    #[inline]
    fn add_line_entry(&mut self) {
        let line = self.inner.alloc.alloc_str(&self.inner.buffer[..]);
        let existing = self.inner.entries.get_mut(self.group_name).unwrap();
        existing.lines.push(line);
    }

    #[inline]
    pub fn with_label<R>(
        &mut self,
        name: &PName,
        value: &(impl SerializePrometheusLabelValue + ?Sized),
        closure: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.inner.labels.push(name, value);
        let r = closure(self);
        self.inner.labels.pop();
        r
    }

    #[inline]
    pub fn with_name<R>(&mut self, name: &PName, closure: impl FnOnce(&mut Self) -> R) -> R {
        self.inner.name.push(name);
        let r = closure(self);
        self.inner.name.pop();
        r
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricType {
    Counter,
    Gauge,
    Histogram,
    Summary,
    Untyped,
}

impl Display for MetricType {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match *self {
            Self::Counter => "counter",
            Self::Gauge => "gauge",
            Self::Histogram => "histogram",
            Self::Summary => "summary",
            Self::Untyped => "untyped",
        };
        f.write_str(name)
    }
}

#[inline]
fn write_label(
    name: &PName,
    value: &(impl SerializePrometheusLabelValue + ?Sized),
    out: &mut impl fmt::Write,
    has_labels: bool,
) -> fmt::Result {
    let sep = if has_labels { ", " } else { "{" };
    write!(out, r#"{sep}{name}=""#)?;
    value.serialize_prometheus_label_value(out)?;
    out.write_char('"')?;
    Ok(())
}

pub trait SerializePrometheusLabelValue {
    fn serialize_prometheus_label_value<W: fmt::Write>(&self, write: &mut W) -> fmt::Result;
}

impl<T: SerializeGoFloat> SerializePrometheusLabelValue for T {
    #[inline]
    fn serialize_prometheus_label_value<W: fmt::Write>(&self, write: &mut W) -> fmt::Result {
        self.serialize_go_float(write)
    }
}

impl SerializePrometheusLabelValue for str {
    #[inline]
    fn serialize_prometheus_label_value<W: fmt::Write>(&self, write: &mut W) -> fmt::Result {
        write!(write, "{}", escape_prometheus_str(self))
    }
}

#[derive(Debug, Default, Clone)]
pub struct LabelBuilder {
    buf: String,
    waypoints: Vec<usize>,
}

impl LabelBuilder {
    #[inline]
    pub const fn new() -> Self {
        Self {
            buf: String::new(),
            waypoints: Vec::new(),
        }
    }

    #[inline]
    pub fn push(&mut self, name: &PName, value: &(impl SerializePrometheusLabelValue + ?Sized)) {
        self.waypoints.push(self.buf.len());
        let has_labels = !self.buf.is_empty();
        write_label(name, value, &mut self.buf, has_labels).unwrap()
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

impl Display for LabelBuilder {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return Ok(());
        }
        write!(f, "{}}}", self.buf)
    }
}

pub trait PrometheusHelpTextSource {
    fn write_help_text(self, buf: &mut String);
}

impl<F> PrometheusHelpTextSource for F
where
    F: FnOnce(&mut String),
{
    fn write_help_text(self, buf: &mut String) {
        self(buf);
    }
}

impl PrometheusHelpTextSource for &str {
    fn write_help_text(self, buf: &mut String) {
        buf.push_str(self);
    }
}
