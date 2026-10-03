//! Finds *where* a project pins a toolchain version — the exact byte range
//! of the version text in `pom.xml`, `build.gradle[.kts]`, or the Maven
//! Wrapper properties — so an update is a minimal, formatting-preserving
//! edit of that text and nothing else.

use std::ops::Range;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Component {
    Jdk,
    /// Maven installed on this machine (`mvn` on PATH).
    Maven,
    /// The project's pinned Maven version (`mvnw`).
    MavenWrapper,
    SpringBoot,
    JavaFx,
}

impl Component {
    pub fn label(self) -> &'static str {
        match self {
            Component::Jdk => "JDK",
            Component::Maven => "Maven",
            Component::MavenWrapper => "Maven Wrapper",
            Component::SpringBoot => "Spring Boot",
            Component::JavaFx => "JavaFX",
        }
    }

    /// Stable key for the "ignore this version" list.
    pub fn key(self) -> &'static str {
        match self {
            Component::Jdk => "jdk",
            Component::Maven => "maven",
            Component::MavenWrapper => "maven-wrapper",
            Component::SpringBoot => "spring-boot",
            Component::JavaFx => "javafx",
        }
    }
}

/// Everywhere one file pins a component's version. A project that pins
/// Spring Boot on the starter *and* the plugin (or JavaFX on several
/// modules) gets all of them updated together, never left mismatched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionSite {
    pub component: Component,
    pub file: PathBuf,
    /// Byte ranges containing the version text, in file order (for the
    /// Maven Wrapper, the whole `distributionUrl` value).
    pub ranges: Vec<Range<usize>>,
    pub current: String,
}

/// All version sites in a workspace's build files.
pub fn find_sites(workspace: &Path) -> Vec<VersionSite> {
    let mut sites = Vec::new();
    let mut add = |file: &Path, found: Vec<Site>| {
        sites.extend(found.into_iter().map(|(component, ranges, current)| VersionSite {
            component,
            file: file.to_path_buf(),
            ranges,
            current,
        }));
    };
    let pom = workspace.join("pom.xml");
    if let Ok(text) = std::fs::read_to_string(&pom) {
        add(&pom, pom_sites(&text));
    }
    for name in ["build.gradle.kts", "build.gradle"] {
        let file = workspace.join(name);
        if let Ok(text) = std::fs::read_to_string(&file) {
            add(&file, gradle_sites(&text));
            break;
        }
    }
    let wrapper = workspace.join(".mvn").join("wrapper").join("maven-wrapper.properties");
    if let Ok(text) = std::fs::read_to_string(&wrapper)
        && let Some((range, current)) = maven_wrapper_site(&text)
    {
        add(&wrapper, vec![(Component::MavenWrapper, vec![range], current)]);
    }
    sites
}

type Site = (Component, Vec<Range<usize>>, String);

/// Groups candidate `(range, version)` pins into one site: the version
/// of `preferred` if given (a parent POM decides), else the most common
/// one; every pin with that version is included.
fn group_site(component: Component, pins: Vec<(Range<usize>, String)>, preferred: Option<String>) -> Option<Site> {
    let pins: Vec<_> = pins
        .into_iter()
        .filter(|(_, v)| crate::version::Version::parse(v).is_some())
        .collect();
    let version = preferred.filter(|p| pins.iter().any(|(_, v)| v == p)).or_else(|| {
        let mut counts: Vec<(&String, usize)> = Vec::new();
        for (_, v) in &pins {
            match counts.iter_mut().find(|(c, _)| *c == v) {
                Some(entry) => entry.1 += 1,
                None => counts.push((v, 1)),
            }
        }
        counts.into_iter().max_by_key(|(_, n)| *n).map(|(v, _)| v.clone())
    })?;
    let mut ranges: Vec<Range<usize>> = pins
        .into_iter()
        .filter(|(_, v)| *v == version)
        .map(|(r, _)| r)
        .collect();
    ranges.sort_by_key(|r| r.start);
    ranges.dedup();
    Some((component, ranges, version))
}

/// Spring Boot and JavaFX versions in a POM. `${property}` references are
/// followed to the `<properties>` entry, since that's the text to edit.
pub fn pom_sites(xml: &str) -> Vec<Site> {
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return Vec::new();
    };
    let root = doc.root_element();
    let properties: Vec<(String, Range<usize>, String)> = child(root, "properties")
        .map(|props| {
            props
                .children()
                .filter(|c| c.is_element())
                .filter_map(|c| {
                    let text = c.first_child().filter(|t| t.is_text())?;
                    Some((c.tag_name().name().to_string(), text.range(), text.text()?.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    let property = |name: &str| {
        properties
            .iter()
            .find(|(n, _, _)| n == name)
            .map(|(_, r, v)| (trimmed_range(xml, r.clone()), v.trim().to_string()))
    };
    // A literal version stays where it is; `${name}` resolves to the
    // property's own text.
    let resolve = |(range, value): (Range<usize>, String)| -> Option<(Range<usize>, String)> {
        let trimmed = value.trim();
        match trimmed.strip_prefix("${").and_then(|s| s.strip_suffix('}')) {
            Some(name) => property(name),
            None => Some((trimmed_range(xml, range), trimmed.to_string())),
        }
    };
    let pinned = |group: &str, artifact_ok: &dyn Fn(&str) -> bool| -> Vec<(Range<usize>, String)> {
        root.descendants()
            .filter(|n| n.has_tag_name("dependency") || n.has_tag_name("plugin"))
            .filter(|n| {
                text_of(child(*n, "groupId")) == Some(group)
                    && text_of(child(*n, "artifactId")).is_some_and(artifact_ok)
            })
            .filter_map(version_text)
            .filter_map(resolve)
            .collect()
    };

    let mut sites = Vec::new();

    // Spring Boot: the parent POM (which then decides the version), every
    // org.springframework.boot dependency/plugin/BOM pinned explicitly,
    // and a spring-boot.version property.
    let mut boot = pinned("org.springframework.boot", &|_| true);
    let mut parent_version = None;
    if let Some(parent) = child(root, "parent")
        && text_of(child(parent, "artifactId")) == Some("spring-boot-starter-parent")
        && let Some(pin) = version_text(parent).and_then(resolve)
    {
        parent_version = Some(pin.1.clone());
        boot.push(pin);
    }
    boot.extend(property("spring-boot.version"));
    if let Some(site) = group_site(Component::SpringBoot, boot, parent_version) {
        sites.push(site);
    }

    // JavaFX: the org.openjfx libraries (not the javafx-maven-plugin, which
    // has its own unrelated version) and javafx.version / openjfx.version.
    let mut javafx = pinned("org.openjfx", &|a| a.starts_with("javafx-") && a != "javafx-maven-plugin");
    javafx.extend(property("javafx.version"));
    javafx.extend(property("openjfx.version"));
    if let Some(site) = group_site(Component::JavaFx, javafx, None) {
        sites.push(site);
    }
    sites
}

/// Shrinks a range to exclude surrounding whitespace.
fn trimmed_range(text: &str, range: Range<usize>) -> Range<usize> {
    let slice = &text[range.clone()];
    let start = range.start + (slice.len() - slice.trim_start().len());
    let end = range.end - (slice.len() - slice.trim_end().len());
    start..end.max(start)
}

/// Spring Boot / JavaFX versions in a Gradle build script (Groovy or
/// Kotlin DSL): the Spring Boot plugin, `group:artifact:version` coordinate
/// strings, and the `javafx { version = … }` block.
pub fn gradle_sites(script: &str) -> Vec<Site> {
    let mut boot = Vec::new();
    let mut javafx = Vec::new();
    let mut offset = 0;
    let mut in_javafx_block = false;
    for line in script.split_inclusive('\n') {
        let code = line.split("//").next().unwrap_or(line);
        let shift = |(r, v): (Range<usize>, String)| (offset + r.start..offset + r.end, v);
        if code.contains("id")
            && code.contains("org.springframework.boot\"") | code.contains("org.springframework.boot'")
            && let Some(pin) = quoted_after(code, "version")
        {
            boot.push(shift(pin));
        }
        boot.extend(coordinate_versions(code, "org.springframework.boot:").into_iter().map(shift));
        javafx.extend(coordinate_versions(code, "org.openjfx:javafx-").into_iter().map(shift));
        let trimmed = code.trim_start();
        if trimmed.starts_with("javafx") && code.contains('{') {
            in_javafx_block = true;
        }
        if in_javafx_block {
            if trimmed.starts_with("version")
                && let Some(pin) = quoted_after(code, "version")
            {
                javafx.push(shift(pin));
            }
            if code.contains('}') {
                in_javafx_block = false;
            }
        }
        offset += line.len();
    }
    [group_site(Component::SpringBoot, boot, None), group_site(Component::JavaFx, javafx, None)]
        .into_iter()
        .flatten()
        .collect()
}

/// Versions inside `"group:artifact:VERSION"` coordinate strings whose
/// coordinate starts with `prefix`.
fn coordinate_versions(line: &str, prefix: &str) -> Vec<(Range<usize>, String)> {
    let mut found = Vec::new();
    let mut search = 0;
    while let Some(pos) = line[search..].find(prefix) {
        let start = search + pos;
        let end = line[start..]
            .find(['"', '\''])
            .map_or(line.len(), |e| start + e);
        let coordinate = &line[start..end];
        if let Some(colon) = coordinate.rfind(':')
            && coordinate.matches(':').count() == 2
        {
            let version_start = start + colon + 1;
            found.push((version_start..end, line[version_start..end].to_string()));
        }
        search = end.max(start + 1);
    }
    found
}

fn child<'a, 'input>(node: roxmltree::Node<'a, 'input>, name: &str) -> Option<roxmltree::Node<'a, 'input>> {
    node.children().find(|c| c.is_element() && c.has_tag_name(name))
}

fn text_of<'a>(node: Option<roxmltree::Node<'a, '_>>) -> Option<&'a str> {
    node.and_then(|n| n.text()).map(str::trim)
}

/// A `<version>` child's text node — its byte range is what an update
/// rewrites.
fn version_text(node: roxmltree::Node<'_, '_>) -> Option<(Range<usize>, String)> {
    let version = child(node, "version")?;
    let text = version.first_child().filter(|t| t.is_text())?;
    Some((text.range(), text.text()?.to_string()))
}

/// The first quoted string after `keyword` in `line`, with its range.
fn quoted_after(line: &str, keyword: &str) -> Option<(Range<usize>, String)> {
    let after = line.find(keyword)? + keyword.len();
    let rest = &line[after..];
    let quote_pos = rest.find(['"', '\''])?;
    let quote = rest.as_bytes()[quote_pos] as char;
    let start = after + quote_pos + 1;
    let len = line[start..].find(quote)?;
    Some((start..start + len, line[start..start + len].to_string()))
}

/// `distributionUrl=…/apache-maven/3.9.6/apache-maven-3.9.6-bin.zip`.
pub fn maven_wrapper_site(properties: &str) -> Option<(Range<usize>, String)> {
    let mut offset = 0;
    for line in properties.split_inclusive('\n') {
        if let Some(value) = line.trim_start().strip_prefix("distributionUrl=") {
            let value = value.trim_end();
            let start = offset + line.find("distributionUrl=")? + "distributionUrl=".len();
            let version = value
                .split("/apache-maven/")
                .nth(1)?
                .split('/')
                .next()?
                .to_string();
            crate::version::Version::parse(&version)?;
            return Some((start..start + value.len(), version));
        }
        offset += line.len();
    }
    None
}

/// Rewrites one site to `new_version`, after checking the file still says
/// what the check saw (so a file edited since is never clobbered). The
/// previous content is kept as `<file>.rji-backup`. Returns the backup path.
pub fn apply_update(site: &VersionSite, new_version: &str) -> anyhow::Result<PathBuf> {
    let text = std::fs::read_to_string(&site.file).with_context(|| format!("reading {}", site.file.display()))?;
    // Every pin must still say what the check saw before anything changes.
    for range in &site.ranges {
        if !text.get(range.clone()).is_some_and(|s| s.contains(&site.current)) {
            bail!("{} changed since the update check — check again", site.file.display());
        }
    }
    if site.ranges.is_empty() {
        bail!("nothing to update in {}", site.file.display());
    }
    // Rewrite back-to-front so earlier ranges' offsets stay valid.
    let mut updated = text.clone();
    let mut ranges = site.ranges.clone();
    ranges.sort_by_key(|r| std::cmp::Reverse(r.start));
    for range in ranges {
        let replaced = updated[range.clone()].replace(&site.current, new_version);
        updated.replace_range(range, &replaced);
    }

    let backup = backup_path(&site.file);
    std::fs::write(&backup, &text).with_context(|| format!("writing backup {}", backup.display()))?;
    std::fs::write(&site.file, updated).with_context(|| format!("writing {}", site.file.display()))?;
    Ok(backup)
}

pub fn backup_path(file: &Path) -> PathBuf {
    let mut name = file.file_name().unwrap_or_default().to_os_string();
    name.push(".rji-backup");
    file.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    const POM: &str = r#"<?xml version="1.0"?>
<project>
  <parent>
    <groupId>org.springframework.boot</groupId>
    <artifactId>spring-boot-starter-parent</artifactId>
    <version>3.3.0</version>
  </parent>
  <properties>
    <javafx.version> 21.0.1 </javafx.version>
  </properties>
  <dependencies>
    <dependency>
      <groupId>org.openjfx</groupId>
      <artifactId>javafx-controls</artifactId>
      <version>${javafx.version}</version>
    </dependency>
  </dependencies>
  <build><plugins><plugin>
    <groupId>org.openjfx</groupId>
    <artifactId>javafx-maven-plugin</artifactId>
    <version>0.0.8</version>
  </plugin></plugins></build>
</project>"#;

    /// The text at every range of a site.
    fn texts<'a>(src: &'a str, site: &Site) -> Vec<&'a str> {
        site.1.iter().map(|r| &src[r.clone()]).collect()
    }

    #[test]
    fn finds_pom_sites_following_properties() {
        let sites = pom_sites(POM);
        let boot = sites.iter().find(|s| s.0 == Component::SpringBoot).unwrap();
        assert_eq!((texts(POM, boot), boot.2.as_str()), (vec!["3.3.0"], "3.3.0"));
        let fx = sites.iter().find(|s| s.0 == Component::JavaFx).unwrap();
        // Resolved to the property, trimmed — not the ${…} reference, and
        // not the javafx-maven-plugin's own version.
        assert_eq!((texts(POM, fx), fx.2.as_str()), (vec!["21.0.1"], "21.0.1"));
    }

    #[test]
    fn every_explicit_spring_boot_pin_is_one_site() {
        // Starter *and* plugin pinned (no parent): both must move together.
        let pom = "<project><dependencies><dependency><groupId>org.springframework.boot</groupId>\
                   <artifactId>spring-boot-starter</artifactId><version>3.2.0</version></dependency></dependencies>\
                   <build><plugins><plugin><groupId>org.springframework.boot</groupId>\
                   <artifactId>spring-boot-maven-plugin</artifactId><version>3.2.0</version></plugin>\
                   <plugin><groupId>org.openjfx</groupId><artifactId>javafx-maven-plugin</artifactId>\
                   <version>0.0.8</version></plugin></plugins></build></project>";
        let sites = pom_sites(pom);
        assert_eq!(sites.len(), 1, "javafx-maven-plugin alone isn't a JavaFX version");
        assert_eq!(texts(pom, &sites[0]), vec!["3.2.0", "3.2.0"]);
    }

    #[test]
    fn parent_version_wins_over_a_stray_pin() {
        let pom = "<project><parent><groupId>org.springframework.boot</groupId>\
                   <artifactId>spring-boot-starter-parent</artifactId><version>3.3.0</version></parent>\
                   <dependencies><dependency><groupId>org.springframework.boot</groupId>\
                   <artifactId>spring-boot-devtools</artifactId><version>3.1.0</version></dependency></dependencies></project>";
        let site = &pom_sites(pom)[0];
        assert_eq!((texts(pom, site), site.2.as_str()), (vec!["3.3.0"], "3.3.0"));
    }

    #[test]
    fn gradle_kotlin_and_groovy() {
        let kts = "plugins {\n    id(\"org.springframework.boot\") version \"3.3.0\"\n}\njavafx {\n    version = \"21\"\n    modules(\"javafx.controls\")\n}\n";
        let sites = gradle_sites(kts);
        assert_eq!(sites.len(), 2);
        assert_eq!(texts(kts, &sites[0]), vec!["3.3.0"]);
        assert_eq!(texts(kts, &sites[1]), vec!["21"]);
        let groovy = "plugins {\n  id 'org.springframework.boot' version '3.1.5'\n}\n\
                      dependencies {\n  implementation 'org.springframework.boot:spring-boot-starter-web:3.1.5'\n\
                      implementation \"org.openjfx:javafx-controls:21.0.2\"\n}\n";
        let sites = gradle_sites(groovy);
        assert_eq!(texts(groovy, &sites[0]), vec!["3.1.5", "3.1.5"]);
        assert_eq!(texts(groovy, &sites[1]), vec!["21.0.2"]);
    }

    #[test]
    fn maven_wrapper() {
        let props = "wrapperVersion=3.3.2\ndistributionUrl=https://repo.maven.apache.org/maven2/org/apache/maven/apache-maven/3.9.6/apache-maven-3.9.6-bin.zip\n";
        let (range, version) = maven_wrapper_site(props).unwrap();
        assert_eq!(version, "3.9.6");
        assert!(props[range].ends_with("apache-maven-3.9.6-bin.zip"));
    }

    #[test]
    fn apply_rewrites_only_the_site_and_keeps_backup() {
        let dir = std::env::temp_dir().join(format!("rji-updates-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".mvn/wrapper")).unwrap();
        std::fs::write(dir.join("pom.xml"), POM).unwrap();
        std::fs::write(
            dir.join(".mvn/wrapper/maven-wrapper.properties"),
            "distributionUrl=https://x/apache-maven/3.9.6/apache-maven-3.9.6-bin.zip\n",
        )
        .unwrap();

        let sites = find_sites(&dir);
        assert_eq!(sites.len(), 3);
        let boot = sites.iter().find(|s| s.component == Component::SpringBoot).unwrap();
        let backup = apply_update(boot, "3.3.5").unwrap();
        let updated = std::fs::read_to_string(dir.join("pom.xml")).unwrap();
        assert_eq!(updated, POM.replace("<version>3.3.0</version>", "<version>3.3.5</version>"));
        assert_eq!(std::fs::read_to_string(backup).unwrap(), POM);
        // The old site no longer matches: a second apply is refused.
        assert!(apply_update(boot, "3.3.6").is_err());

        // Multi-pin site: every pin moves together.
        let two = "<project><dependencies><dependency><groupId>org.springframework.boot</groupId><artifactId>spring-boot-starter</artifactId><version>3.2.0</version></dependency></dependencies><build><plugins><plugin><groupId>org.springframework.boot</groupId><artifactId>spring-boot-maven-plugin</artifactId><version>3.2.0</version></plugin></plugins></build></project>";
        std::fs::write(dir.join("pom.xml"), two).unwrap();
        let site = find_sites(&dir).into_iter().find(|s| s.component == Component::SpringBoot).unwrap();
        apply_update(&site, "3.2.12").unwrap();
        let updated = std::fs::read_to_string(dir.join("pom.xml")).unwrap();
        assert_eq!(updated, two.replace("3.2.0", "3.2.12"));

        let wrapper = sites.iter().find(|s| s.component == Component::MavenWrapper).unwrap();
        apply_update(wrapper, "3.9.9").unwrap();
        let props = std::fs::read_to_string(dir.join(".mvn/wrapper/maven-wrapper.properties")).unwrap();
        assert!(props.contains("/apache-maven/3.9.9/apache-maven-3.9.9-bin.zip"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
