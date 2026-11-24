// build

// Compiles `snap.rc` so the exe carries its icon.

fn main() {
	println!("cargo:rerun-if-changed=icon.ico");
	embed_resource::compile("snap.rc", embed_resource::NONE)
		.manifest_required()
		.unwrap();
}
