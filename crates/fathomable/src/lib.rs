#![forbid(unsafe_code)]
//! Core library for fathomable.

/// Return this package's canonical name.
#[must_use]
pub fn package_name() -> &'static str {
    "fathomable"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_package_name() {
        assert_eq!(package_name(), "fathomable");
    }
}
