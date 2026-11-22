use std::{env, fs, path::PathBuf};

const ICON: &[u8] = include_bytes!("assets/GitLite.ico");
const MANIFEST: &[u8] = include_bytes!("assets/GitLite.manifest");

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let (machine, rva_relocation) = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86_64") => (0x8664, 0x0003),
        Ok("x86") => (0x014c, 0x0007),
        Ok("aarch64") => (0xaa64, 0x0002),
        _ => panic!("Unsupported Windows architecture"),
    };
    let path = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("gitlite-resources.obj");
    let resources = app_resources().into_coff(machine, rva_relocation);
    fs::write(&path, resources).expect("write Windows resources");
    println!("cargo:rerun-if-changed=assets/GitLite.ico");
    println!("cargo:rerun-if-changed=assets/GitLite.manifest");
    println!("cargo:rerun-if-changed=build.rs");
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        // The application manifest is already part of the resource object.
        println!("cargo:rustc-link-arg-bins=/MANIFEST:NO");
    }
    println!("cargo:rustc-link-arg-bins={}", path.display());
}

fn app_resources() -> Resources {
    const RT_ICON: u32 = 3;
    const RT_GROUP_ICON: u32 = 14;
    const RT_MANIFEST: u32 = 24;

    let image_count = u16::from_le_bytes(ICON[4..6].try_into().unwrap());
    let mut group_icon = ICON[..6].to_vec();
    let mut resources = Resources::default();
    let root = resources.directory(3);
    let icons = resources.child(root, 0, RT_ICON, image_count);
    let groups = resources.child(root, 1, RT_GROUP_ICON, 1);
    let manifests = resources.child(root, 2, RT_MANIFEST, 1);

    for index in 0..image_count {
        let entry = 6 + index as usize * 16;
        let size = read_u32(ICON, entry + 8) as usize;
        let offset = read_u32(ICON, entry + 12) as usize;
        let id = index + 1;
        // A group icon substitutes a 16-bit resource ID for the ICO's file offset.
        group_icon.extend_from_slice(&ICON[entry..entry + 12]);
        put_u16(&mut group_icon, id);
        resources.add(icons, index, id.into(), &ICON[offset..offset + size]);
    }
    resources.add(groups, 0, 1, &group_icon);
    resources.add(manifests, 0, 1, MANIFEST);
    resources
}

// The PE resource directory uses three numeric levels: type, ID, and language.
// The two COFF sections let the linker resolve the absolute image-relative data
// addresses without requiring a separate Windows resource compiler.
#[derive(Default)]
struct Resources {
    directories: Vec<u8>,
    data: Vec<u8>,
    relocations: Vec<(u32, u32)>,
}

impl Resources {
    fn directory(&mut self, entries: u16) -> usize {
        let offset = self.directories.len();
        self.directories
            .resize(offset + 16 + usize::from(entries) * 8, 0);
        self.directories[offset + 14..offset + 16].copy_from_slice(&entries.to_le_bytes());
        offset
    }

    fn entry(&mut self, directory: usize, index: u16, id: u32, offset: u32) {
        let start = directory + 16 + usize::from(index) * 8;
        self.directories[start..start + 4].copy_from_slice(&id.to_le_bytes());
        self.directories[start + 4..start + 8].copy_from_slice(&offset.to_le_bytes());
    }

    fn child(&mut self, directory: usize, index: u16, id: u32, count: u16) -> usize {
        let offset = self.directory(count);
        self.entry(directory, index, id, 0x8000_0000 | offset as u32);
        offset
    }

    fn add(&mut self, kinds: usize, index: u16, id: u32, bytes: &[u8]) {
        const EN_US: u32 = 0x0409;

        let language = self.child(kinds, index, id, 1);
        let data_entry = self.directories.len() as u32;
        self.entry(language, 0, EN_US, data_entry);
        put_u32(&mut self.directories, 0); // RVA filled in by the linker.
        put_u32(&mut self.directories, bytes.len() as u32);
        self.directories.extend_from_slice(&[0; 8]); // Code page and reserved.

        self.relocations.push((data_entry, self.data.len() as u32));
        self.data.extend_from_slice(bytes);
        self.data.resize(self.data.len().next_multiple_of(8), 0);
    }

    fn into_coff(self, machine: u16, rva_relocation: u16) -> Vec<u8> {
        const HEADER_SIZE: u32 = 20 + 2 * 40;
        const RELOCATION_SIZE: u32 = 10;
        let directory_size = self.directories.len() as u32;
        let data_size = self.data.len() as u32;
        let relocation_count = self.relocations.len() as u32;
        let relocation_offset = HEADER_SIZE + directory_size;
        let data_offset =
            (relocation_offset + relocation_count * RELOCATION_SIZE).next_multiple_of(4);
        let symbol_offset = data_offset + data_size;

        let mut object = Vec::new();
        put_u16(&mut object, machine);
        put_u16(&mut object, 2);
        put_u32(&mut object, 0); // Deterministic timestamp.
        put_u32(&mut object, symbol_offset);
        put_u32(&mut object, relocation_count);
        put_u16(&mut object, 0); // No optional header.
        put_u16(&mut object, 0);
        section(
            &mut object,
            b".rsrc$01",
            directory_size,
            HEADER_SIZE,
            relocation_offset,
            relocation_count as u16,
        );
        section(&mut object, b".rsrc$02", data_size, data_offset, 0, 0);
        object.extend_from_slice(&self.directories);

        for (index, (place, _)) in self.relocations.iter().enumerate() {
            put_u32(&mut object, *place);
            put_u32(&mut object, index as u32);
            put_u16(&mut object, rva_relocation);
        }
        object.resize(data_offset as usize, 0);
        object.extend_from_slice(&self.data);

        for (index, (_, offset)) in self.relocations.iter().enumerate() {
            let name = format!("$R{index:06}");
            object.extend_from_slice(name.as_bytes());
            put_u32(&mut object, *offset);
            put_u16(&mut object, 2); // The payload is in .rsrc$02.
            put_u16(&mut object, 0);
            object.extend_from_slice(&[3, 0]); // Static symbol, no auxiliary entries.
        }
        put_u32(&mut object, 4); // Empty COFF string table.
        object
    }
}

fn section(
    object: &mut Vec<u8>,
    name: &[u8; 8],
    size: u32,
    offset: u32,
    relocation_offset: u32,
    relocation_count: u16,
) {
    object.extend_from_slice(name);
    object.extend_from_slice(&[0; 8]); // Virtual size and address.
    put_u32(object, size);
    put_u32(object, offset);
    put_u32(object, relocation_offset);
    put_u32(object, 0); // No line number table.
    put_u16(object, relocation_count);
    put_u16(object, 0);
    put_u32(object, 0x4030_0040); // Read-only initialized data, aligned to 4 bytes.
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn put_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}
