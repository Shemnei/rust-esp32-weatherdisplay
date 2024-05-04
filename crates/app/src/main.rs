use std::ffi::CString;
use std::time::Duration;

use esp_idf_hal::peripherals::Peripherals;
use esp_idf_svc::eventloop::EspEventLoop;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{
	BlockingWifi, ClientConfiguration, Configuration, EspWifi,
};
use esp_idf_sys::esp;

fn get_device_service_name() -> CString {
	const PREFIX: &str = "PROV_";
	let mut eth_mac = [0u8; 6];

	unsafe {
		esp!(esp_idf_sys::esp_wifi_get_mac(
			esp_idf_sys::wifi_interface_t_WIFI_IF_STA,
			eth_mac.as_mut_ptr()
		))
		.unwrap()
	};

	CString::new(format!(
		"{PREFIX}{:02X}{:02X}{:02x}",
		eth_mac[3], eth_mac[4], eth_mac[5]
	))
	.unwrap()
}

fn main() {
	// It is necessary to call this function once. Otherwise some patches to the runtime
	// implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
	esp_idf_svc::sys::link_patches();

	// Bind the log crate to the ESP Logging facilities
	esp_idf_svc::log::EspLogger::initialize_default();

	let nvs = EspDefaultNvsPartition::take().unwrap();
	let sys_loop = EspEventLoop::take().unwrap();
	let peripherals = Peripherals::take().unwrap();

	let mut esp_wifi =
		EspWifi::new(peripherals.modem, sys_loop.clone(), Some(nvs.clone()))
			.unwrap();
	let mut wifi =
		BlockingWifi::wrap(&mut esp_wifi, sys_loop.clone()).unwrap();

	wifi.set_configuration(&Configuration::Client(ClientConfiguration {
		..Default::default()
	}))
	.unwrap();

	unsafe {
		let cfg = esp_idf_sys::wifi_prov_mgr_config_t {
			scheme: esp_idf_sys::wifi_prov_scheme_ble,
			scheme_event_handler: esp_idf_sys::wifi_prov_event_handler_t {
				event_cb: Some(
					esp_idf_sys::wifi_prov_scheme_ble_event_cb_free_btdm,
				),
				user_data: std::ptr::null_mut(),
			},
			app_event_handler: esp_idf_sys::wifi_prov_event_handler_t {
				event_cb: None,
				user_data: std::ptr::null_mut(),
			},
		};

		esp!(esp_idf_sys::wifi_prov_mgr_init(cfg)).unwrap();
		let mut provisioned = false;
		esp!(esp_idf_sys::wifi_prov_mgr_is_provisioned(&mut provisioned))
			.unwrap();

		log::info!("Provisioned: {provisioned}");

		if !provisioned {
			/* This step is only useful when scheme is wifi_prov_scheme_ble. This will
			 * set a custom 128 bit UUID which will be included in the BLE advertisement
			 * and will correspond to the primary GATT service that provides provisioning
			 * endpoints as GATT characteristics. Each GATT characteristic will be
			 * formed using the primary service UUID as base, with different auto assigned
			 * 12th and 13th bytes (assume counting starts from 0th byte). The client side
			 * applications must identify the endpoints by reading the User Characteristic
			 * Description descriptor (0x2901) for each characteristic, which contains the
			 * endpoint name of the characteristic */
			let mut custom_service_uuid = [
				/* LSB <---------------------------------------
				 * ---------------------------------------> MSB */
				0xb4, 0xdf, 0x5a, 0x1c, 0x3f, 0x6b, 0xf4, 0xbf, 0xea, 0x4a,
				0x82, 0x03, 0x04, 0x90, 0x1a, 0x02,
			];

			/* If your build fails with linker errors at this point, then you may have
			 * forgotten to enable the BT stack or BTDM BLE settings in the SDK (e.g. see
			 * the sdkconfig.defaults in the example project) */
			esp!(esp_idf_sys::wifi_prov_scheme_ble_set_service_uuid(
				custom_service_uuid.as_mut_ptr()
			))
			.unwrap();

			// TESTING - Allow reprov
			esp!(esp_idf_sys::wifi_prov_mgr_disable_auto_stop(1000)).unwrap();

			let sec = esp_idf_sys::wifi_prov_security_WIFI_PROV_SECURITY_1;
			let pop = CString::new("test123").unwrap();

			let service_name = get_device_service_name();

			esp!(esp_idf_sys::wifi_prov_mgr_start_provisioning(
				sec,
				pop.as_ptr() as _,
				service_name.as_ptr(),
				std::ptr::null()
			))
			.unwrap();

			esp_idf_sys::wifi_prov_mgr_wait();
		}
	}

	loop {
		std::thread::park();
		std::thread::sleep(Duration::from_secs(1));
	}
}
