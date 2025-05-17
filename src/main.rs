use adb_client::{ADBDeviceExt, ADBServer, ADBUSBDevice};
fn main() {
    let vendor_id = 0x22d9;
    let product_id = 0x2765;
    let mut device = ADBUSBDevice::new(vendor_id, product_id).expect("cannot find device");
    let mut server = ADBServer::default();
    let devices = server.devices();

    println!("devices: {devices:?}");
    let mut device = server.get_device().expect("cannot get device");

    println!("Hello, world!");
}
