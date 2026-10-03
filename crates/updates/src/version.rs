//! Version parsing and comparison for Java-ecosystem release numbers
//! (`3.3.5`, `25.0.2`, `21`, `3.4.0-RC1`, `6.0.0.Final`, `21-ea+3`).

use std::cmp::Ordering;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    parts: Vec<u64>,
    pub raw: String,
}

impl Version {
    /// Parses the leading numeric `a.b.c…` components; `None` if there are
    /// none. A JDK build suffix (`+10`) is ignored.
    pub fn parse(raw: &str) -> Option<Version> {
        let trimmed = raw.trim().trim_start_matches(['v', 'V']);
        let numeric: String = trimmed
            .split(['-', '+', '_'])
            .next()?
            .split('.')
            .take_while(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
            .collect::<Vec<_>>()
            .join(".");
        if numeric.is_empty() {
            return None;
        }
        let parts = numeric.split('.').filter_map(|p| p.parse().ok()).collect();
        Some(Version {
            parts,
            raw: raw.trim().to_string(),
        })
    }

    pub fn major(&self) -> u64 {
        self.parts.first().copied().unwrap_or(0)
    }

    pub fn minor(&self) -> u64 {
        self.parts.get(1).copied().unwrap_or(0)
    }

    fn part(&self, i: usize) -> u64 {
        self.parts.get(i).copied().unwrap_or(0)
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        let len = self.parts.len().max(other.parts.len());
        (0..len)
            .map(|i| self.part(i).cmp(&other.part(i)))
            .find(|o| o.is_ne())
            .unwrap_or(Ordering::Equal)
    }
}

/// A final release, not a milestone / release candidate / snapshot /
/// early-access build. Only stable releases are ever suggested.
pub fn is_stable(raw: &str) -> bool {
    let lower = raw.to_ascii_lowercase();
    let Some(version) = Version::parse(raw) else {
        return false;
    };
    let numeric_len = version.parts.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(".").len();
    let rest = lower.trim_start_matches('v').get(numeric_len..).unwrap_or("");
    // JDK build metadata (`+10`) and the classic "this is final" markers
    // are fine; anything else (-rc1, -m2, -alpha, -snapshot, -ea, …) isn't.
    rest.is_empty()
        || rest.starts_with('+')
        || matches!(rest, ".release" | ".final" | "-ga" | ".ga")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Bump {
    Patch,
    Minor,
    Major,
}

/// How big a jump `current -> candidate` is; `None` if it's not newer.
pub fn classify(current: &Version, candidate: &Version) -> Option<Bump> {
    if candidate <= current {
        return None;
    }
    Some(if candidate.major() != current.major() {
        Bump::Major
    } else if candidate.minor() != current.minor() {
        Bump::Minor
    } else {
        Bump::Patch
    })
}

/// The newest stable version in `versions`.
pub fn latest_stable<'a>(versions: impl IntoIterator<Item = &'a str>) -> Option<Version> {
    versions
        .into_iter()
        .filter(|v| is_stable(v))
        .filter_map(Version::parse)
        .max()
}

/// The newest stable version on the same `major.minor` line as `current`
/// — the only kind of update applied automatically (bug/security fixes).
pub fn latest_patch<'a>(versions: impl IntoIterator<Item = &'a str>, current: &Version) -> Option<Version> {
    versions
        .into_iter()
        .filter(|v| is_stable(v))
        .filter_map(Version::parse)
        .filter(|v| v.major() == current.major() && v.minor() == current.minor() && v > current)
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn parses_and_orders() {
        assert!(v("3.10.0") > v("3.9.9"));
        assert_eq!(v("21").cmp(&v("21.0.0")), Ordering::Equal);
        assert_eq!(v("25.0.2+10").major(), 25);
        assert!(Version::parse("abc").is_none());
    }

    #[test]
    fn stability() {
        for stable in ["3.3.5", "25.0.2+10", "5.3.9.RELEASE", "6.0.0.Final", "21"] {
            assert!(is_stable(stable), "{stable}");
        }
        for unstable in ["3.4.0-RC1", "3.4.0-M2", "1.0-SNAPSHOT", "22-ea+3", "2.0.0-alpha1", "3.0.0-beta"] {
            assert!(!is_stable(unstable), "{unstable}");
        }
    }

    #[test]
    fn bumps_and_patch_line() {
        assert_eq!(classify(&v("3.3.0"), &v("3.3.5")), Some(Bump::Patch));
        assert_eq!(classify(&v("3.3.0"), &v("3.5.0")), Some(Bump::Minor));
        assert_eq!(classify(&v("3.3.0"), &v("4.0.0")), Some(Bump::Major));
        assert_eq!(classify(&v("3.3.0"), &v("3.3.0")), None);
        let all = ["3.3.0", "3.3.4", "3.3.5", "3.3.6-RC1", "3.4.0", "4.0.0-M1"];
        assert_eq!(latest_stable(all).unwrap().raw, "3.4.0");
        assert_eq!(latest_patch(all, &v("3.3.0")).unwrap().raw, "3.3.5");
        assert!(latest_patch(all, &v("3.4.0")).is_none());
    }
}
