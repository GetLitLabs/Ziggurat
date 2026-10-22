//! T1 shared vocabulary for Ziggurat. `no_std`, no allocation.
#![no_std]
#![forbid(unsafe_code)]

use core::fmt;

/// Routing tier for every dispatch. Wire value is `1..=5`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum TerraceLevel {
    Foundation = 1,
    Substrate = 2,
    Workforce = 3,
    Crucible = 4,
    Pinnacle = 5,
}

impl TerraceLevel {
    pub const fn as_u8(self) -> u8 {
        self as u8
    }
}

impl TryFrom<u8> for TerraceLevel {
    type Error = TypesError;

    fn try_from(level: u8) -> Result<Self, Self::Error> {
        match level {
            1 => Ok(Self::Foundation),
            2 => Ok(Self::Substrate),
            3 => Ok(Self::Workforce),
            4 => Ok(Self::Crucible),
            5 => Ok(Self::Pinnacle),
            other => Err(TypesError::TerraceOutOfRange(other)),
        }
    }
}

/// A BSM-C namespace such as `seed.scaffold.emit`. Must contain a dot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BsmcNamespace<'a>(&'a str);

impl<'a> BsmcNamespace<'a> {
    pub fn parse(raw: &'a str) -> Result<Self, TypesError> {
        let dotted = raw.contains('.');
        let well_formed = raw.split('.').all(|seg| !seg.is_empty());
        if dotted && well_formed {
            Ok(Self(raw))
        } else {
            Err(TypesError::NamespaceWithoutDot)
        }
    }

    pub fn as_str(&self) -> &'a str {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypesError {
    TerraceOutOfRange(u8),
    NamespaceWithoutDot,
}

impl fmt::Display for TypesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TerraceOutOfRange(level) => {
                write!(f, "terrace_level {level} is outside 1..=5")
            }
            Self::NamespaceWithoutDot => {
                f.write_str("bsmc_namespace must be dot-separated non-empty segments")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terrace_accepts_one_through_five() {
        for level in 1u8..=5 {
            assert_eq!(
                TerraceLevel::try_from(level).map(TerraceLevel::as_u8),
                Ok(level)
            );
        }
    }

    #[test]
    fn terrace_rejects_zero_and_six() {
        assert_eq!(
            TerraceLevel::try_from(0),
            Err(TypesError::TerraceOutOfRange(0))
        );
        assert_eq!(
            TerraceLevel::try_from(6),
            Err(TypesError::TerraceOutOfRange(6))
        );
    }

    #[test]
    fn namespace_requires_dot() {
        assert_eq!(
            BsmcNamespace::parse("seed.scaffold.emit").map(|ns| ns.as_str()),
            Ok("seed.scaffold.emit")
        );
        for bad in ["seed", "", ".", "seed.", ".emit", "seed..emit"] {
            assert_eq!(
                BsmcNamespace::parse(bad),
                Err(TypesError::NamespaceWithoutDot),
                "{bad}"
            );
        }
    }
}
