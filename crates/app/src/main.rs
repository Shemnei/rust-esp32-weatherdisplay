use std::ffi::CString;
use std::net::Ipv4Addr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use esp_idf_hal::io::Write;
use esp_idf_hal::peripherals::Peripherals;
use esp_idf_svc::eventloop::EspEventLoop;
use esp_idf_svc::http::server::{
	self, fn_handler, ChainRoot, Connection, EspHttpServer, Handler,
	Middleware,
};
use esp_idf_svc::http::Method;
use esp_idf_svc::ipv4::{self, RouterConfiguration};
use esp_idf_svc::netif::{EspNetif, NetifConfiguration, NetifStack};
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{
	AccessPointConfiguration, AuthMethod, BlockingWifi, ClientConfiguration,
	Configuration, EspWifi,
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

fn ble_provision() {
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

fn scan() {
	let sys_loop = EspEventLoop::take().unwrap();
	let peripherals = Peripherals::take().unwrap();

	let mut esp_wifi =
		EspWifi::new(peripherals.modem, sys_loop.clone(), None).unwrap();
	let mut wifi =
		BlockingWifi::wrap(&mut esp_wifi, sys_loop.clone()).unwrap();
	wifi.set_configuration(&Configuration::Client(ClientConfiguration {
		..Default::default()
	}))
	.unwrap();
	wifi.start().unwrap();

	loop {
		if let Ok(aps) = wifi.scan_n::<8>() {
			println!(
				"---------------------------------------------------------"
			);
			for ap in aps.0 {
				println!("{ap:?}");
			}
		} else {
			log::error!("Failed to scan for WiFi APs");
		}
		std::thread::sleep(Duration::from_secs(5));
	}
}

fn captivity() {
	let sys_loop = EspEventLoop::take().unwrap();
	let peripherals = Peripherals::take().unwrap();

	let netif = EspNetif::new_with_conf(&NetifConfiguration {
		ip_configuration: ipv4::Configuration::Router(RouterConfiguration {
			subnet: ipv4::Subnet {
				gateway: Ipv4Addr::new(192, 168, 6, 1),
				mask: ipv4::Mask(24),
			},
			dhcp_enabled: true,
			dns: Some(Ipv4Addr::new(1, 1, 1, 1)),
			secondary_dns: None,
		}),
		stack: NetifStack::Ap,
		key: heapless::String::try_from("test123").unwrap(),
		description: heapless::String::try_from("ap").unwrap(),
		route_priority: 10,
		custom_mac: None,
	})
	.unwrap();

	let mut esp_wifi =
		EspWifi::new(peripherals.modem, sys_loop.clone(), None).unwrap();
	let _ = esp_wifi.swap_netif_ap(netif).unwrap();

	let mut wifi =
		BlockingWifi::wrap(&mut esp_wifi, sys_loop.clone()).unwrap();
	wifi.set_configuration(&Configuration::AccessPoint(
		AccessPointConfiguration {
			ssid: heapless::String::try_from("test123").unwrap(),
			auth_method: AuthMethod::WPA2Personal,
			password: heapless::String::try_from("hello_world_foo_bar_123")
				.unwrap(),
			..Default::default()
		},
	))
	.unwrap();
	wifi.start().unwrap();
	wifi.wait_netif_up().unwrap();

	// Server

	pub struct CooldownMiddleware {
		cooldown: Duration,
		last_request: Mutex<Instant>,
	}

	impl CooldownMiddleware {
		pub fn with_cooldown(cooldown: Duration) -> Self {
			Self { cooldown, last_request: Mutex::new(Instant::now()) }
		}
	}

	impl<C, H> Middleware<C, H> for CooldownMiddleware
	where
		C: Connection,
		H: Handler<C>,
	{
		type Error = ();

		fn handle(
			&self,
			connection: &mut C,
			handler: &H,
		) -> Result<(), Self::Error> {
			log::info!("Called {}", connection.uri());

			let now = Instant::now();
			let mut last_request = self.last_request.lock().unwrap();
			let valid = if last_request.elapsed() > self.cooldown {
				// Set before handler call to prevent multiple simultaneous requests
				*last_request = Instant::now();
				true
			} else {
				false
			};
			drop(last_request);

			if valid {
				handler.handle(connection).map_err(|_| ())?;
			} else {
				log::warn!("Too many requests - Blocking");
				connection
					.initiate_response(429, None, &[])
					.map_err(|_| ())?;
			}

			log::info!("\t>> Took: {:?}", now.elapsed());

			Ok(())
		}
	}

	let mut server = EspHttpServer::new(&server::Configuration {
		stack_size: 9000,
		..Default::default()
	})
	.unwrap();

	server
		.handler(
			"/",
			Method::Get,
			CooldownMiddleware::with_cooldown(Duration::from_secs(5)).compose(
				fn_handler(move |req| {
					let mut res = req
						.into_response(
							200,
							None,
							&[("Content-Type", "text/html; charset=utf-8")],
						)
						.map_err(|_| ())?;
					res.write_all(
						br#"
					<html>
						<body>
							<h1>Hello World</h1>
						</body>
					</html>
				"#,
					)
					.map_err(|_| ())?;

					Result::<(), ()>::Ok(())
				}),
			),
		)
		.unwrap();

	loop {
		std::thread::park();
		std::thread::sleep(Duration::from_secs(1));
	}
}

fn main() {
	// It is necessary to call this function once. Otherwise some patches to the runtime
	// implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
	esp_idf_svc::sys::link_patches();

	// Bind the log crate to the ESP Logging facilities
	esp_idf_svc::log::EspLogger::initialize_default();

	captivity();
}
