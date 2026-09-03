//! Field declares a single input. The form is generated from these, so a new
//! tool gets a complete, validated UI for free.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Selects the editor the form renders for a field.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FieldKind {
    /// A free-text input.
    Text,
    /// A set of choices.
    Select,
    /// A switch.
    Bool,
}

/// Marks a field's semantic purpose so the UI can wire tools together without
/// knowing what any particular tool does.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Role {
    #[default]
    None,
    /// The field holding the host/network being acted on. A result row
    /// selected in one tool is handed off to another tool's target field.
    Target,
    /// A port specification, so a row that names a port can be handed into it.
    Ports,
    /// The switch that says a run adds to what earlier runs of the same job
    /// found and does not replace it. The interface keeps the table rather
    /// than clearing it, and the tool is told the rows that are already there.
    Keep,
}

/// One choice in a `Select`.
#[derive(Clone, Debug)]
pub struct Opt {
    pub value: String,
    pub label: String,
    pub desc: String,
}

impl Opt {
    pub fn new(value: &str, label: &str, desc: &str) -> Opt {
        Opt { value: value.into(), label: label.into(), desc: desc.into() }
    }
}

/// A check run on the raw value when the form is submitted. Returning an error
/// blocks the run and shows the message.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Validator {
    Required,
    IntRange(i64, i64),
    /// A Go-style duration, or a bare millisecond count.
    Duration,
    /// A port specification.
    Ports,
    /// A domain usable as a certificate-transparency query.
    CtDomain,
    /// A transfer rate: `2MB`, `500KB`, `1.5MB/s`, or empty for no limit.
    Rate,
    /// What a good HTTP answer is: `any`, `2xx`, `404`, `200-204`, or a list.
    Expect,
}

impl Validator {
    pub fn check(self, s: &str) -> Result<(), String> {
        let s = s.trim();
        match self {
            Validator::Required => {
                if s.is_empty() {
                    Err("required".into())
                } else {
                    Ok(())
                }
            }
            Validator::IntRange(lo, hi) => {
                if s.is_empty() {
                    return Ok(());
                }
                let n: i64 = s.parse().map_err(|_| "must be a number".to_string())?;
                if n < lo || n > hi {
                    Err(format!("must be between {lo} and {hi}"))
                } else {
                    Ok(())
                }
            }
            Validator::Duration => {
                if s.is_empty() || parse_duration(s).is_some() {
                    Ok(())
                } else {
                    Err("use 500ms, 2s, or a plain number of milliseconds".into())
                }
            }
            Validator::Ports => crate::net::ports::parse_ports(s).map(|_| ()),
            Validator::CtDomain => crate::net::ct::normalize_domain(s).map(|_| ()),
            Validator::Expect => crate::tools::http::valid_expect(s),
            Validator::Rate => {
                if s.is_empty() || parse_rate(s).is_some() {
                    Ok(())
                } else {
                    Err("use 2MB, 500KB, or leave it blank for no limit".into())
                }
            }
        }
    }
}

/// Resolves shorthand in a field's value into something explicit and editable.
/// Tab runs it: a target of "auto" becomes the concrete subnet, then the
/// concrete range. It sees the other field values too, so an expansion can
/// depend on them: "auto" means a different network once an interface is
/// chosen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Expand {
    Target,
    Ports,
    Resolver,
    CtDomain,
    /// A destination directory, with `~` spelled out.
    Downloads,
}

impl Expand {
    /// Returns the expanded value, or `None` when there was nothing to expand.
    pub fn apply(self, value: &str, p: &Params) -> Option<String> {
        match self {
            Expand::Target => crate::net::target::expand_target_spec(value, &p.str("iface")),
            Expand::Ports => crate::net::ports::expand_port_spec(value),
            Expand::Resolver => {
                if !value.trim().is_empty() {
                    return None;
                }
                crate::net::dns::system_resolvers().into_iter().next()
            }
            Expand::CtDomain => crate::net::ct::expand_domain(value),
            Expand::Downloads => {
                let expanded = crate::dl::names::expand_home(value);
                let expanded = expanded.display().to_string();
                (expanded != value.trim()).then_some(expanded)
            }
        }
    }
}

/// Hides a field unless it applies. Hidden fields keep their value but are
/// skipped during navigation and validation, which is how tools show
/// protocol-specific options only when relevant.
#[derive(Clone, Debug)]
pub enum VisibleIf {
    Equals(&'static str, &'static str),
    NotEquals(&'static str, &'static str),
}

impl VisibleIf {
    pub fn eval(&self, p: &Params) -> bool {
        match self {
            VisibleIf::Equals(k, v) => p.str(k) == *v,
            VisibleIf::NotEquals(k, v) => p.str(k) != *v,
        }
    }
}

/// Declares a single input.
#[derive(Clone, Debug)]
pub struct Field {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: FieldKind,
    pub role: Role,
    /// The initial value. For `Select` it must match an option value; for
    /// `Bool` it is "true" or "false".
    pub default: String,
    /// Ghost text shown in an empty text field.
    pub placeholder: String,
    /// The one-line hint shown under the field.
    pub help: &'static str,
    pub options: Vec<Opt>,
    pub validate: Option<Validator>,
    pub expand: Option<Expand>,
    pub visible_if: Option<VisibleIf>,
}

impl Field {
    pub fn text(key: &'static str, label: &'static str, help: &'static str) -> Field {
        Field {
            key,
            label,
            kind: FieldKind::Text,
            role: Role::None,
            default: String::new(),
            placeholder: String::new(),
            help,
            options: Vec::new(),
            validate: None,
            expand: None,
            visible_if: None,
        }
    }

    pub fn select(
        key: &'static str,
        label: &'static str,
        help: &'static str,
        default: &str,
        options: Vec<Opt>,
    ) -> Field {
        Field {
            kind: FieldKind::Select,
            default: default.into(),
            options,
            ..Field::text(key, label, help)
        }
    }

    pub fn boolean(key: &'static str, label: &'static str, help: &'static str, default: bool) -> Field {
        Field {
            kind: FieldKind::Bool,
            default: if default { "true".into() } else { "false".into() },
            ..Field::text(key, label, help)
        }
    }

    pub fn role(mut self, role: Role) -> Field {
        self.role = role;
        self
    }
    pub fn default(mut self, v: &str) -> Field {
        self.default = v.into();
        self
    }
    pub fn placeholder(mut self, v: &str) -> Field {
        self.placeholder = v.into();
        self
    }
    pub fn validate(mut self, v: Validator) -> Field {
        self.validate = Some(v);
        self
    }
    pub fn expand(mut self, e: Expand) -> Field {
        self.expand = Some(e);
        self
    }
    pub fn visible_if(mut self, v: VisibleIf) -> Field {
        self.visible_if = Some(v);
        self
    }

    /// Reports whether the field applies given the current values.
    pub fn visible(&self, p: &Params) -> bool {
        self.visible_if.as_ref().is_none_or(|v| v.eval(p))
    }

}

/// The raw string values collected by the form, keyed by `Field::key`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Params(BTreeMap<String, String>);

impl Params {
    pub fn new() -> Params {
        Params::default()
    }

    /// Builds a `Params` pre-populated from a field list.
    pub fn defaults(fields: &[Field]) -> Params {
        let mut p = Params::new();
        for f in fields {
            p.set(f.key, &f.default);
        }
        p
    }

    pub fn set(&mut self, key: &str, value: &str) {
        self.0.insert(key.to_string(), value.to_string());
    }

    pub fn raw(&self, key: &str) -> &str {
        self.0.get(key).map(String::as_str).unwrap_or("")
    }

    /// The trimmed string value.
    pub fn str(&self, key: &str) -> String {
        self.raw(key).trim().to_string()
    }

    pub fn bool(&self, key: &str) -> bool {
        matches!(self.str(key).as_str(), "true" | "1" | "yes" | "on")
    }

    pub fn int(&self, key: &str, def: i64) -> i64 {
        self.str(key).parse().unwrap_or(def)
    }

    pub fn usize(&self, key: &str, def: usize) -> usize {
        self.int(key, def as i64).max(0) as usize
    }

    /// Parses a duration such as "500ms" or "1s", falling back to `def`. A
    /// bare number is read as milliseconds so users can type "250" and mean
    /// what they expect.
    pub fn dur(&self, key: &str, def: Duration) -> Duration {
        parse_duration(&self.str(key)).unwrap_or(def)
    }
}

/// Parses a transfer rate into bytes per second: `2MB`, `500KB`, `1.5MB/s`, a
/// bare number of bytes. The trailing `/s` is optional because a rate is the
/// only thing this field could mean.
pub fn parse_rate(s: &str) -> Option<u64> {
    let s = s.trim().trim_end_matches("/s").trim_end_matches("ps").trim();
    if s.is_empty() {
        return None;
    }
    let digits = s.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    let unit = s[digits.len()..].trim().to_ascii_lowercase();
    let value: f64 = digits.trim().parse().ok()?;
    if value <= 0.0 {
        return None;
    }
    let scale = match unit.as_str() {
        "" | "b" => 1.0,
        "k" | "kb" | "kib" => 1024.0,
        "m" | "mb" | "mib" => 1024.0 * 1024.0,
        "g" | "gb" | "gib" => 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some((value * scale) as u64)
}

/// Parses a Go-flavoured duration: `500ms`, `1.5s`, `2m`, or a bare
/// millisecond count.
pub fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(n) = s.parse::<u64>() {
        return Some(Duration::from_millis(n));
    }

    let (num, unit) = s.split_at(s.find(|c: char| c.is_alphabetic())?);
    let value: f64 = num.trim().parse().ok()?;
    if value < 0.0 {
        return None;
    }
    let seconds = match unit.trim() {
        "ns" => value / 1e9,
        "us" | "µs" => value / 1e6,
        "ms" => value / 1e3,
        "s" => value,
        "m" => value * 60.0,
        "h" => value * 3600.0,
        _ => return None,
    };
    // `from_secs_f64` panics on a value it cannot represent, and a unit
    // multiplies whatever was typed: `1e300h` is infinite by the time it gets
    // here, and a number need only be large to overflow. A duration nobody
    // could wait out is not worth taking the program down for.
    Duration::try_from_secs_f64(seconds).ok()
}


#[cfg(test)]
mod duration_tests {
    use super::parse_duration;

    #[test]
    fn a_duration_nobody_could_wait_out_is_refused_and_not_fatal() {
        // A unit multiplies whatever was typed, so a large number becomes an
        // impossible one and an enormous number becomes infinite. Neither is
        // worth taking the program down for.
        assert_eq!(parse_duration("1e300h"), None);
        assert_eq!(parse_duration("1e30s"), None);
        assert_eq!(parse_duration("-1s"), None);
        // The ordinary ones still read the way they are written.
        assert_eq!(parse_duration("500ms"), Some(std::time::Duration::from_millis(500)));
        assert_eq!(parse_duration("2m"), Some(std::time::Duration::from_secs(120)));
    }
}

#[cfg(test)]
mod rate_tests {
    use super::{Validator, parse_rate};

    #[test]
    fn a_rate_is_read_the_way_it_is_written() {
        assert_eq!(parse_rate("2MB"), Some(2 * 1024 * 1024));
        assert_eq!(parse_rate("500KB"), Some(500 * 1024));
        assert_eq!(parse_rate("1.5MB/s"), Some(1_572_864));
        // A bare number is bytes, which is the only thing it could be.
        assert_eq!(parse_rate("4096"), Some(4096));
    }

    #[test]
    fn a_blank_limit_is_no_limit_rather_than_an_error() {
        assert_eq!(parse_rate(""), None);
        assert!(Validator::Rate.check("").is_ok());
        assert!(Validator::Rate.check("2MB").is_ok());
        assert!(Validator::Rate.check("fast").is_err());
        assert!(Validator::Rate.check("0").is_err());
    }
}
