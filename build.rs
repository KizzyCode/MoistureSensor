fn main() {
    // Recompile if linker script changed
    println!("cargo:rerun-if-changed=../memory.x");
}
