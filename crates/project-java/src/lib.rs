//! Maven/Spring Boot/JavaFX project detection, run tasks, templates, and
//! JDK discovery.

pub mod gradle;
pub mod java_source;
pub mod jdk_detect;
pub mod maven;
pub mod project;
pub mod templates;

pub use gradle::{GradleProject, detect_gradle_project};
pub use jdk_detect::{JdkCandidate, JdkSource, detect_jdks};
pub use maven::{MavenProject, detect_maven_project};
pub use project::{
    BuildSystem, MainClass, MainKind, ProjectInfo, Task, TaskGroup, default_run_task,
    detect_project, project_tasks,
};
