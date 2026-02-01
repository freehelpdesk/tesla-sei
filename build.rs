fn main() {
    prost_build::Config::new()
        .default_package_filename("dashcam")
        .compile_protos(&["proto/dashcam.proto"], &["proto"])
        .expect("prost-build failed");
}
