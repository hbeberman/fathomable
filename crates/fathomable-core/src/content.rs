// @okf-doc: /decisions/0026-binary-files-and-file-info.md
//! What a file's bytes are: text, binary, or too large to read.
//!
//! The rule is git's own, so the viewer and `git diff` agree on every
//! file. The `diff` attribute of `.gitattributes` decides when it is set
//! ([`Attr`]); otherwise a file is binary when a `NUL` byte appears in its
//! first [`SNIFF_BYTES`] bytes, git's `buffer_is_binary` heuristic
//! ([`is_binary`]). A text file over the [`Policy`] ceiling is not read
//! at all. [`Format`] names the common binary formats by their magic
//! numbers for the file-info pane, and [`human_size`] formats byte counts
//! the way that pane shows them.
//!
//! # Examples
//!
//! ```
//! use fathomable_core::content::{Attr, Format, is_binary};
//!
//! assert!(is_binary(b"\0asm\x01\0\0\0"));
//! assert!(!is_binary("caf\u{e9}\n".as_bytes()));
//! assert_eq!(Format::sniff(b"\0asm\x01\0\0\0").map(|f| f.to_string()),
//!            Some("WebAssembly module".to_owned()));
//! // `-diff` wins over the bytes, as it does for git.
//! assert_eq!(Attr::Binary.classify(b"plain text\n"), Some(true));
//! ```

use std::fmt;

/// How many leading bytes the `NUL` sniff looks at, as git does.
pub const SNIFF_BYTES: usize = 8000;

/// The `diff` attribute of a path, as `.gitattributes` sets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Attr {
    /// `diff` is unset (`-diff`, or the `binary` macro): always binary.
    Binary,
    /// `diff` is set, or names a driver: always text.
    Text,
    /// Nothing said; the bytes decide.
    #[default]
    Unspecified,
}

impl Attr {
    /// Whether `bytes` are binary under this attribute: the attribute's
    /// answer when it has one, else the [`is_binary`] sniff.
    #[must_use]
    pub fn classify(self, bytes: &[u8]) -> Option<bool> {
        match self {
            Self::Binary => Some(true),
            Self::Text => Some(false),
            Self::Unspecified => Some(is_binary(bytes)),
        }
    }

    /// The answer the attribute gives on its own, before any bytes are
    /// read.
    #[must_use]
    pub const fn decided(self) -> Option<bool> {
        match self {
            Self::Binary => Some(true),
            Self::Text => Some(false),
            Self::Unspecified => None,
        }
    }
}

/// git's heuristic: a `NUL` in the first [`SNIFF_BYTES`] bytes.
#[must_use]
pub fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(SNIFF_BYTES).any(|&byte| byte == 0)
}

/// How a file is read: its diff attribute and the size ceiling for text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Policy {
    /// The path's `diff` attribute.
    pub attr: Attr,
    /// The largest text file that is read, in bytes.
    pub max_bytes: u64,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            attr: Attr::Unspecified,
            max_bytes: DEFAULT_MAX_MIB * MIB,
        }
    }
}

/// Bytes in a mebibyte.
pub const MIB: u64 = 1024 * 1024;

/// The default `viewer.max-file-size-mib`: roomy enough for any log or
/// lock file, small enough that a stray dump cannot stall the viewer.
pub(crate) const DEFAULT_MAX_MIB: u64 = 64;

/// What a loaded file holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    /// UTF-8 text.
    Text(String),
    /// A binary file, never read whole.
    Binary {
        /// Size on disk in bytes.
        size: u64,
        /// The format the magic table recognises, if any.
        format: Option<Format>,
    },
    /// A text file over the policy's ceiling, not read.
    TooLarge {
        /// Size on disk in bytes.
        size: u64,
        /// The ceiling it exceeded, in bytes.
        max_bytes: u64,
    },
}

impl Content {
    /// The text, when there is one.
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            Self::Binary { .. } | Self::TooLarge { .. } => None,
        }
    }

    /// Whether this is [`Content::Binary`].
    #[must_use]
    pub const fn is_binary(&self) -> bool {
        matches!(self, Self::Binary { .. })
    }
}

/// A binary format the file-info pane can name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Format {
    /// A WebAssembly module.
    WebAssembly,
    /// An ELF executable, object, or shared library.
    Elf,
    /// A Mach-O executable or library.
    MachO,
    /// `MZ`: a Windows executable or DLL.
    Pe,
    /// `CAFEBABE` opens both a Java class and a Mach-O fat binary.
    JavaClassOrMachOFat,
    /// A PNG image.
    Png,
    /// A JPEG image.
    Jpeg,
    /// A GIF image.
    Gif,
    /// A WebP image.
    WebP,
    /// A Windows bitmap image.
    Bmp,
    /// A Windows icon.
    Ico,
    /// A PDF document.
    Pdf,
    /// A gzip stream.
    Gzip,
    /// A Zstandard stream.
    Zstd,
    /// An xz stream.
    Xz,
    /// A bzip2 stream.
    Bzip2,
    /// A zip archive, which also covers jar, docx, and other zip-based files.
    Zip,
    /// A 7-Zip archive.
    SevenZip,
    /// A tar archive.
    Tar,
    /// A `SQLite` database.
    Sqlite,
    /// A WOFF web font.
    Woff,
    /// A WOFF2 web font.
    Woff2,
    /// A TrueType font.
    TrueType,
    /// An OpenType font.
    OpenType,
    /// An MP3 audio stream.
    Mp3,
    /// An Ogg container.
    Ogg,
    /// A FLAC audio stream.
    Flac,
    /// A WAV audio file.
    Wav,
    /// An MP4 container.
    Mp4,
}

impl Format {
    /// The format whose magic number opens `bytes`, if any.
    #[must_use]
    pub fn sniff(bytes: &[u8]) -> Option<Self> {
        let starts = |magic: &[u8]| bytes.starts_with(magic);
        let riff = |kind: &[u8]| starts(b"RIFF") && bytes.get(8..12) == Some(kind);
        let format = if starts(b"\0asm") {
            Self::WebAssembly
        } else if starts(b"\x7fELF") {
            Self::Elf
        } else if starts(b"\xfe\xed\xfa\xce")
            || starts(b"\xfe\xed\xfa\xcf")
            || starts(b"\xcf\xfa\xed\xfe")
            || starts(b"\xce\xfa\xed\xfe")
        {
            Self::MachO
        } else if starts(b"\xca\xfe\xba\xbe") {
            Self::JavaClassOrMachOFat
        } else if starts(b"MZ") {
            Self::Pe
        } else if starts(b"\x89PNG\r\n\x1a\n") {
            Self::Png
        } else if starts(b"\xff\xd8\xff") {
            Self::Jpeg
        } else if starts(b"GIF87a") || starts(b"GIF89a") {
            Self::Gif
        } else if riff(b"WEBP") {
            Self::WebP
        } else if riff(b"WAVE") {
            Self::Wav
        } else if starts(b"BM") {
            Self::Bmp
        } else if starts(b"\0\0\x01\0") {
            Self::Ico
        } else if starts(b"%PDF") {
            Self::Pdf
        } else if starts(b"\x1f\x8b") {
            Self::Gzip
        } else if starts(b"\x28\xb5\x2f\xfd") {
            Self::Zstd
        } else if starts(b"\xfd7zXZ\0") {
            Self::Xz
        } else if starts(b"BZh") {
            Self::Bzip2
        } else if starts(b"PK\x03\x04") || starts(b"PK\x05\x06") {
            Self::Zip
        } else if starts(b"7z\xbc\xaf\x27\x1c") {
            Self::SevenZip
        } else if bytes.get(257..262) == Some(b"ustar") {
            Self::Tar
        } else if starts(b"SQLite format 3\0") {
            Self::Sqlite
        } else if starts(b"wOFF") {
            Self::Woff
        } else if starts(b"wOF2") {
            Self::Woff2
        } else if starts(b"\0\x01\0\0") {
            Self::TrueType
        } else if starts(b"OTTO") {
            Self::OpenType
        } else if starts(b"ID3") || starts(b"\xff\xfb") {
            Self::Mp3
        } else if starts(b"OggS") {
            Self::Ogg
        } else if starts(b"fLaC") {
            Self::Flac
        } else if bytes.get(4..8) == Some(b"ftyp") {
            Self::Mp4
        } else {
            return None;
        };
        Some(format)
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::WebAssembly => "WebAssembly module",
            Self::Elf => "ELF executable or object",
            Self::MachO => "Mach-O executable",
            Self::Pe => "Windows executable (PE)",
            Self::JavaClassOrMachOFat => "Java class or Mach-O fat binary",
            Self::Png => "PNG image",
            Self::Jpeg => "JPEG image",
            Self::Gif => "GIF image",
            Self::WebP => "WebP image",
            Self::Bmp => "BMP image",
            Self::Ico => "ICO icon",
            Self::Pdf => "PDF document",
            Self::Gzip => "gzip archive",
            Self::Zstd => "zstd archive",
            Self::Xz => "xz archive",
            Self::Bzip2 => "bzip2 archive",
            Self::Zip => "zip archive",
            Self::SevenZip => "7-Zip archive",
            Self::Tar => "tar archive",
            Self::Sqlite => "SQLite database",
            Self::Woff => "WOFF font",
            Self::Woff2 => "WOFF2 font",
            Self::TrueType => "TrueType font",
            Self::OpenType => "OpenType font",
            Self::Mp3 => "MP3 audio",
            Self::Ogg => "Ogg container",
            Self::Flac => "FLAC audio",
            Self::Wav => "WAV audio",
            Self::Mp4 => "MP4 container",
        })
    }
}

/// `bytes` as the file-info pane shows it: `1.5 MiB`, `312 B`.
///
/// Whole units are shown without a fraction (`64 MiB`); anything else
/// keeps one decimal.
#[must_use]
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut unit = 0;
    let mut size = bytes;
    while size >= 1024 && unit < UNITS.len() - 1 {
        size /= 1024;
        unit += 1;
    }
    if unit == 0 {
        return format!("{bytes} B");
    }
    let scale = 1024u64.pow(u32::try_from(unit).unwrap_or(0));
    if bytes.is_multiple_of(scale) {
        format!("{size} {}", UNITS[unit])
    } else {
        #[expect(
            clippy::cast_precision_loss,
            reason = "one decimal of a human-readable size"
        )]
        let value = bytes as f64 / scale as f64;
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// The smallest power-of-two MiB count that fits `bytes`, for the
/// notice that suggests a new `max-file-size-mib`.
#[must_use]
pub fn suggested_max_mib(bytes: u64) -> u64 {
    bytes.div_ceil(MIB).max(1).next_power_of_two()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff_stops_at_the_git_window() {
        let mut late = vec![b'a'; SNIFF_BYTES];
        late.push(0);
        assert!(!is_binary(&late));
        late[SNIFF_BYTES - 1] = 0;
        assert!(is_binary(&late));
    }

    #[test]
    fn attribute_overrides_bytes_both_ways() {
        assert_eq!(Attr::Text.classify(b"\0\0"), Some(false));
        assert_eq!(Attr::Binary.classify(b"hi"), Some(true));
        assert_eq!(Attr::Unspecified.classify(b"hi"), Some(false));
        assert_eq!(Attr::Unspecified.decided(), None);
    }

    #[test]
    fn sizes_round_and_suggest_powers_of_two() {
        assert_eq!(human_size(312), "312 B");
        assert_eq!(human_size(64 * MIB), "64 MiB");
        assert_eq!(human_size(1_552_850), "1.5 MiB");
        assert_eq!(suggested_max_mib(0), 1);
        assert_eq!(suggested_max_mib(64 * MIB + 1), 128);
        assert_eq!(suggested_max_mib(312 * MIB + 5), 512);
    }
}
