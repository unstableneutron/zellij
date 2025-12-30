use std::io::Result;

fn main() -> Result<()> {
    // prost-build outputs to OUT_DIR, file named after proto package
    // For package "zellij.remote.v1", generates "zellij.remote.v1.rs"
    prost_build::compile_protos(&["proto/zellij_remote.proto"], &["proto/"])?;
    Ok(())
}
