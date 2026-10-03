//! Prints what JDK detection finds: `cargo run -p rji-project-java --example jdks`
fn main() {
    for jdk in rji_project_java::detect_jdks() {
        println!("{:?} | version={:?} | {:?}", jdk.home, jdk.version, jdk.source);
    }
}
