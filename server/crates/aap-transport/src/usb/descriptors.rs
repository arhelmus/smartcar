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
///
/// Bit position MUST be 6 — per `include/uapi/linux/usb/functionfs.h`:
///   `FUNCTIONFS_HAS_SS_DESC      = (1 << 2)`
///   `FUNCTIONFS_HAS_MS_OS_DESC   = (1 << 3)`
///   `FUNCTIONFS_VIRTUAL_ADDR     = (1 << 4)`
///   `FUNCTIONFS_EVENTFD          = (1 << 5)`
///   `FUNCTIONFS_ALL_CTRL_RECIP   = (1 << 6)`
///   `FUNCTIONFS_CONFIG0_SETUP    = (1 << 7)`
/// An earlier revision had this as `1 << 4`, which silently sets
/// `FUNCTIONFS_VIRTUAL_ADDR` instead (changes endpoint file naming to
/// `ep_<addr>`) and leaves ALL_CTRL_RECIP off — so the AOAP vendor
/// requests 51/52/53 would never reach ep0 in the initial persona.
const FUNCTIONFS_ALL_CTRL_RECIP: u32 = 1 << 6;

// ── USB descriptor types ──────────────────────────────────────────────────────

const USB_DT_INTERFACE: u8 = 4;
const USB_DT_ENDPOINT: u8 = 5;

// USB class codes used by the initial AOAP-mode-switch persona. We mirror
// a real Pixel in MTP mode (Image class / Still Image subclass / PTP
// protocol) so the car's AA-handler software actually recognises us as
// a phone and bothers to probe for AOAP support via vendor request 51.
// (Audi MMI 2022 enumerated our previous vendor-class-with-no-endpoints
// initial descriptor at the kernel level but never sent the AOAP probe —
// it apparently filters AOAP candidates by interface class / endpoint
// presence, not just VID.)
const USB_CLASS_IMAGE: u8 = 0x06;
const USB_SUBCLASS_STILL_IMAGE: u8 = 0x01;
const USB_PROTOCOL_PTP: u8 = 0x01;

// ── Public builders ───────────────────────────────────────────────────────────

/// Descriptor blob for the initial AOAP negotiation gadget.
///
/// Presents as a Pixel-in-MTP-mode phone: Still Image (PTP) class
/// interface with the three endpoints a real MTP phone exposes — bulk
/// IN, bulk OUT, interrupt IN. The bulk endpoints are never serviced
/// in userspace; they exist solely so the host sees a phone-shaped
/// device and triggers its AOAP-probe path on ep0 (vendor request 51).
///
/// `FUNCTIONFS_ALL_CTRL_RECIP` is set so the device-directed AOAP
/// vendor requests reach ep0 in userspace.
pub fn initial_descriptors() -> Vec<u8> {
    // Three endpoints, same as a real Pixel MTP interface.
    let mut fs = ptp_interface_descriptor(3);
    fs.extend_from_slice(&endpoint_descriptor(0x81, 64)); // EP1 IN  bulk  64 B (FS)
    fs.extend_from_slice(&endpoint_descriptor(0x02, 64)); // EP2 OUT bulk  64 B (FS)
    fs.extend_from_slice(&interrupt_endpoint_descriptor(0x83, 28, 6)); // EP3 IN intr

    let mut hs = ptp_interface_descriptor(3);
    hs.extend_from_slice(&endpoint_descriptor(0x81, 512));
    hs.extend_from_slice(&endpoint_descriptor(0x02, 512));
    hs.extend_from_slice(&interrupt_endpoint_descriptor(0x83, 28, 6));

    descriptor_blob(
        FUNCTIONFS_HAS_FS_DESC | FUNCTIONFS_HAS_HS_DESC | FUNCTIONFS_ALL_CTRL_RECIP,
        &fs,
        &hs,
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
///
/// Layout per `linux/usb/functionfs.h` — **all the per-speed counts come
/// before any descriptor blob**, not interleaved:
///
/// ```text
/// | magic | length | flags | fs_count | hs_count | fs_descs… | hs_descs… |
/// ```
///
/// (Earlier this was `fs_count | fs_descs | hs_count | hs_descs`, which
/// made the kernel parse `hs_count` out of the middle of `fs_descs`,
/// fail the `length == len` check, and reject the blob with EINVAL.)
fn descriptor_blob(flags: u32, fs_descs: &[u8], hs_descs: &[u8]) -> Vec<u8> {
    // header(12) + fs_count(4) + hs_count(4) + fs_descs + hs_descs
    let total = 12u32 + 4 + 4 + fs_descs.len() as u32 + hs_descs.len() as u32;

    let mut buf = Vec::with_capacity(total as usize);
    put_le32(&mut buf, FUNCTIONFS_DESCRIPTORS_MAGIC_V2);
    put_le32(&mut buf, total);
    put_le32(&mut buf, flags);
    put_le32(&mut buf, count_usb_descriptors(fs_descs) as u32);
    put_le32(&mut buf, count_usb_descriptors(hs_descs) as u32);
    buf.extend_from_slice(fs_descs);
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

/// PTP/MTP-class interface for the initial AOAP-mode-switch persona.
/// Class/subclass/protocol match a real Pixel in MTP mode so the host
/// recognises us as a phone.
fn ptp_interface_descriptor(num_endpoints: u8) -> Vec<u8> {
    vec![
        9,                        // bLength
        USB_DT_INTERFACE,         // bDescriptorType
        0,                        // bInterfaceNumber
        0,                        // bAlternateSetting
        num_endpoints,            // bNumEndpoints
        USB_CLASS_IMAGE,          // bInterfaceClass
        USB_SUBCLASS_STILL_IMAGE, // bInterfaceSubClass
        USB_PROTOCOL_PTP,         // bInterfaceProtocol
        1,                        // iInterface → string index 1
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

/// Interrupt endpoint — used by the MTP-like initial persona for the
/// event-notification endpoint a real Pixel exposes (EP3 IN, 28 bytes,
/// poll interval 6 = `2^(6-1)=32` microframes ≈ 4 ms at HS).
fn interrupt_endpoint_descriptor(address: u8, max_packet: u16, interval: u8) -> Vec<u8> {
    let [lo, hi] = max_packet.to_le_bytes();
    vec![
        7,               // bLength
        USB_DT_ENDPOINT, // bDescriptorType
        address,         // bEndpointAddress
        0x03,            // bmAttributes (Interrupt)
        lo,
        hi,       // wMaxPacketSize (LE16)
        interval, // bInterval
    ]
}

fn put_le32(buf: &mut Vec<u8>, val: u32) {
    buf.extend_from_slice(&val.to_le_bytes());
}

fn put_le16(buf: &mut Vec<u8>, val: u16) {
    buf.extend_from_slice(&val.to_le_bytes());
}
