//! Binary descriptor blobs written to FunctionFS ep0.
//!
//! The kernel's FunctionFS expects two sequential writes to ep0 before the
//! gadget can be enabled: a descriptor blob and a strings blob.  Both use
//! little-endian, packed C structs as defined in
//! `include/uapi/linux/usb/functionfs.h`.

// ── Constants (from linux/usb/functionfs.h) ───────────────────────────────────

const FUNCTIONFS_DESCRIPTORS_MAGIC_V2: u32 = 3;
const FUNCTIONFS_STRINGS_MAGIC: u32 = 2;

/// Include full-speed descriptor set.
const FUNCTIONFS_HAS_FS_DESC: u32 = 1 << 0;
/// Include high-speed descriptor set.
const FUNCTIONFS_HAS_HS_DESC: u32 = 1 << 1;
/// Route ALL control transfers (not just interface-directed) to ep0.
/// Required so FunctionFS sees the AOAP vendor requests (device-directed).
const FUNCTIONFS_ALL_CTRL_RECIP: u32 = 1 << 4;

// ── USB descriptor types ──────────────────────────────────────────────────────

const USB_DT_INTERFACE: u8 = 4;
const USB_DT_ENDPOINT: u8 = 5;

// ── Public builders ───────────────────────────────────────────────────────────

/// Descriptor blob for the initial AOAP negotiation gadget.
///
/// No data endpoints — only ep0 for control.  Uses `FUNCTIONFS_ALL_CTRL_RECIP`
/// so FunctionFS ep0 receives the device-directed AOAP vendor requests 51/52/53.
pub fn initial_descriptors() -> Vec<u8> {
    let intf = interface_descriptor(0);
    descriptor_blob(
        FUNCTIONFS_HAS_FS_DESC | FUNCTIONFS_HAS_HS_DESC | FUNCTIONFS_ALL_CTRL_RECIP,
        &intf,
        &intf,
    )
}

/// Descriptor blob for the AOAP accessory gadget.
///
/// Two bulk endpoints: EP1 IN (board→host TX) and EP2 OUT (host→board RX).
pub fn accessory_descriptors() -> Vec<u8> {
    let intf = interface_descriptor(2); // bNumEndpoints = 2

    let mut fs = intf.clone();
    fs.extend_from_slice(&endpoint_descriptor(0x81, 64)); // EP1 IN  64 B (FS)
    fs.extend_from_slice(&endpoint_descriptor(0x02, 64)); // EP2 OUT 64 B (FS)

    let mut hs = intf.clone();
    hs.extend_from_slice(&endpoint_descriptor(0x81, 512)); // EP1 IN  512 B (HS)
    hs.extend_from_slice(&endpoint_descriptor(0x02, 512)); // EP2 OUT 512 B (HS)

    descriptor_blob(FUNCTIONFS_HAS_FS_DESC | FUNCTIONFS_HAS_HS_DESC, &fs, &hs)
}

/// Strings blob: English (0x0409), one string for iInterface.
///
/// The same blob is used for both gadget personas.
pub fn strings() -> Vec<u8> {
    // NUL-terminated interface name.
    const IFACE_STR: &[u8] = b"Android Accessory Interface\0"; // 28 bytes

    // Header(16) + lang_code(2) + string(28) = 46 bytes total.
    let total = 16u32 + 2 + IFACE_STR.len() as u32;

    let mut buf = Vec::with_capacity(total as usize);
    put_le32(&mut buf, FUNCTIONFS_STRINGS_MAGIC);
    put_le32(&mut buf, total);
    put_le32(&mut buf, 1); // str_count
    put_le32(&mut buf, 1); // lang_count
    put_le16(&mut buf, 0x0409); // English
    buf.extend_from_slice(IFACE_STR);
    buf
}

// ── Private helpers ───────────────────────────────────────────────────────────

/// Build a V2 descriptor blob for the given full-speed / high-speed descriptor sets.
fn descriptor_blob(flags: u32, fs_descs: &[u8], hs_descs: &[u8]) -> Vec<u8> {
    // header(12) + fs_count(4) + fs_descs + hs_count(4) + hs_descs
    let total = 12u32 + 4 + fs_descs.len() as u32 + 4 + hs_descs.len() as u32;

    let mut buf = Vec::with_capacity(total as usize);
    put_le32(&mut buf, FUNCTIONFS_DESCRIPTORS_MAGIC_V2);
    put_le32(&mut buf, total);
    put_le32(&mut buf, flags);
    put_le32(&mut buf, count_usb_descriptors(fs_descs) as u32);
    buf.extend_from_slice(fs_descs);
    put_le32(&mut buf, count_usb_descriptors(hs_descs) as u32);
    buf.extend_from_slice(hs_descs);
    buf
}

/// Count the number of USB descriptors in a packed byte sequence.
///
/// Each descriptor starts with `bLength` (the byte length of that descriptor).
fn count_usb_descriptors(blob: &[u8]) -> usize {
    let mut n = 0;
    let mut pos = 0;
    while pos < blob.len() {
        let len = blob[pos] as usize;
        if len == 0 {
            break;
        }
        n += 1;
        pos += len;
    }
    n
}

fn interface_descriptor(num_endpoints: u8) -> Vec<u8> {
    vec![
        9,                // bLength
        USB_DT_INTERFACE, // bDescriptorType
        0,                // bInterfaceNumber
        0,                // bAlternateSetting
        num_endpoints,    // bNumEndpoints
        0xFF,             // bInterfaceClass  (Vendor)
        0xFF,             // bInterfaceSubClass (Vendor)
        0x00,             // bInterfaceProtocol
        1,                // iInterface → string index 1
    ]
}

fn endpoint_descriptor(address: u8, max_packet: u16) -> Vec<u8> {
    let [lo, hi] = max_packet.to_le_bytes();
    vec![
        7,               // bLength
        USB_DT_ENDPOINT, // bDescriptorType
        address,         // bEndpointAddress  (0x81 = EP1 IN, 0x02 = EP2 OUT)
        0x02,            // bmAttributes      (Bulk)
        lo,
        hi, // wMaxPacketSize    (LE16)
        0,  // bInterval         (0 for bulk)
    ]
}

fn put_le32(buf: &mut Vec<u8>, val: u32) {
    buf.extend_from_slice(&val.to_le_bytes());
}

fn put_le16(buf: &mut Vec<u8>, val: u16) {
    buf.extend_from_slice(&val.to_le_bytes());
}
