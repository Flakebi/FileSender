#[macro_use]
extern crate clap;
extern crate glib;
extern crate gdk;
extern crate gtk;
extern crate iron;
extern crate mount;
extern crate multipart;
extern crate params;
extern crate url;

mod gui;

#[cfg(not(feature = "bundled"))]
use std::fs::File;
#[cfg(not(feature = "bundled"))]
use std::io::Read;
use std::net::{IpAddr, ToSocketAddrs};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::vec::Vec;

use clap::{App, AppSettings, Arg};
use iron::prelude::*;
use iron::{Listening, status};
use iron::error::HttpResult;
use iron::headers::{Charset, ContentDisposition, DispositionParam, DispositionType};
use iron::middleware::Handler;
use iron::mime::Mime;
use iron::modifiers::{Header, Redirect};
use mount::Mount;
use multipart::server::{Multipart, MultipartData, MultipartFile};
use params::Params;
use url::Url;

pub struct FileSender {
	upload_file_name: String,
	upload_file_size: u64,
	upload_text: String,
	download_text: String,
	download_files: Vec<PathBuf>,
}

fn validate<T: FromStr>(val: String) -> Result<(), String> where T::Err: std::fmt::Display {
	T::from_str(val.as_str()).map(|_| ()).map_err(|e| format!("{}", e))
}

fn get_filename(path: &PathBuf) -> String {
	path.file_name().map(|p| p.to_string_lossy().to_string()).unwrap_or(String::from("unknown"))
}

// Read files from disk
#[cfg(not(feature = "bundled"))]
fn get_file(path: &str) -> Option<Vec<u8>> {
	File::open(path).ok().and_then(|mut file| {
		let mut content = Vec::new();
		file.read_to_end(&mut content).ok().map(|_| content)
	})
}

// Read files from stored data
macro_rules! bundle {
	($m:expr; $($name:expr),*) => {
		match $m {
			$($name => Some(include_bytes!(concat!("../", $name)).to_vec()),)*
			_ => unreachable!(),
		}
	}
}

#[cfg(feature = "bundled")]
fn get_file(path: &str) -> Option<Vec<u8>> {
	bundle!(path; "Web/index.html", "Web/static/icon.png", "Web/static/style.css",
			"Window.glade")
}

fn get_web_file(path: &str) -> Option<Vec<u8>> {
	get_file(format!("Web/{}", path).as_str())
}

fn get_string(path: &str) -> Option<String> {
	get_file(path).and_then(|content| {
		String::from_utf8(content).ok()
	})
}

fn get_web_string(path: &str) -> Option<String> {
	get_string(format!("Web/{}", path).as_str())
}

fn main() {
	// Get options
	let args = App::new("FileSender")
		.version(crate_version!())
		.author(crate_authors!())
		.about("Send and receive files using a website")
		.global_setting(AppSettings::ColoredHelp)
		.arg(Arg::with_name("address").short("a").long("address")
			 .validator(validate::<IpAddr>)
			 .default_value("0.0.0.0")
			 .help("The address for the server to listen"))
		.arg(Arg::with_name("port").short("p").long("port")
			 .validator(validate::<u16>)
			 .default_value("44333")
			 .help("The port for the server to listen"))
		.arg(Arg::with_name("upload-filename").short("u").long("upload-filename")
			 .default_value("Upload.file")
			 .help("The filename that will be used to save uploaded files"))
		.arg(Arg::with_name("upload-size").short("s").long("upload-size")
			 .default_value("50000000")
			 .validator(validate::<u64>)
			 .help("The maximum size for uploaded files"))
		.get_matches();
	let address = IpAddr::from_str(args.value_of("address").unwrap()).unwrap();
	let port = u16::from_str(args.value_of("port").unwrap()).unwrap();
	let upload_file_name = args.value_of("upload-filename").unwrap();
	let upload_file_size = u64::from_str(args.value_of("upload-size").unwrap()).unwrap();

	let file_sender = Arc::new(Mutex::new(FileSender {
		upload_file_name: upload_file_name.to_string(),
		upload_file_size: upload_file_size,
		upload_text: String::new(),
		download_text: String::new(),
		download_files: Vec::new(),
	}));
	let mut server = start_server((address, port), file_sender.clone()).expect("Could not start server");

	gui::main(file_sender, &server);

	if let Err(error) = server.close() {
		println!("Cannot close server: {:?}", error);
	}
}

struct FileSenderFunctionHandler<F: Send + Sync + 'static + Fn(Arc<Mutex<FileSender>>, &mut Request) -> IronResult<Response>> {
	file_sender: Arc<Mutex<FileSender>>,
	f: F,
}

impl<F: Send + Sync + 'static + Fn(Arc<Mutex<FileSender>>, &mut Request) -> IronResult<Response>> Handler for FileSenderFunctionHandler<F> {
	fn handle(&self, r: &mut Request) -> IronResult<Response> {
		(self.f)(self.file_sender.clone(), r)
	}
}

struct WebFile {
	path: &'static str,
}

impl Handler for WebFile {
	fn handle(&self, _: &mut Request) -> IronResult<Response> {
		match get_web_file(self.path) {
			Some(content) => {
				if self.path.ends_with(".css") {
					Ok(Response::with((status::Ok, "text/css; charset=utf-8".parse::<Mime>().unwrap(), content)))
				} else {
					Ok(Response::with((status::Ok, content)))
				}
			}
			None => Ok(Response::with((status::NotFound, "Not found"))),
		}
	}
}

fn start_server<To: ToSocketAddrs>(addr: To, file_sender: Arc<Mutex<FileSender>>) -> HttpResult<Listening> {
	// Setup server
	let mut mount = Mount::new();
	mount.mount("/static/style.css", WebFile { path: "static/style.css" });
	mount.mount("/static/icon.png", WebFile { path: "static/icon.png" });
	let h = FileSenderFunctionHandler {
		file_sender: file_sender.clone(),
		f: handle_file,
	};
	mount.mount("/data/upload", h);
	let h = FileSenderFunctionHandler {
		file_sender: file_sender.clone(),
		f: handle_file_download,
	};
	mount.mount("/data/download/", h);
	let h = FileSenderFunctionHandler {
		file_sender: file_sender.clone(),
		f: handle_text,
	};
	mount.mount("/data/text", h);
	let h = FileSenderFunctionHandler {
		file_sender: file_sender.clone(),
		f: handle_index,
	};
	mount.mount("/", h);
	let server = try!(Iron::new(mount).http(addr));
	println!("Started server on {}", server.socket);
	Ok(server)
}

fn handle_index(file_sender: Arc<Mutex<FileSender>>, _: &mut Request) -> IronResult<Response> {
	// Read index.html file
	if let Some(mut content) = get_web_string("index.html") {
		{
			let fs = file_sender.lock().unwrap();
			content = content.replace("{download_text}", fs.download_text.as_str());
			content = content.replace("{upload_text}", fs.upload_text.as_str());
			let text = fs.download_files.iter().map(|p| get_filename(p))
				.enumerate().fold(String::new(), |s, (i, p)|
					format!(r###"{}<li><a href="data/download/{i}">{path}</a></li>"###, s, i = i, path = p));
			content = content.replace("{download_files}",
				format!(r###"<ul class="file-list">{}</ul>"###, text).as_str());
		}
		Ok(Response::with((status::Ok, "text/html; charset=utf-8".parse::<Mime>().unwrap(), content)))
	} else {
		Ok(Response::with((status::NotFound, "File not found")))
	}
}

fn handle_text(file_sender: Arc<Mutex<FileSender>>, request: &mut Request) -> IronResult<Response> {
	if request.method != iron::method::Method::Post {
		Ok(Response::with((status::BadRequest, "Uploaded data not found (not a post request)")))
	} else {
		match Multipart::from_request(request) {
			Ok(_) => Ok(Response::with((status::BadRequest, "This should not be a multipart request"))),
			Err(request) => {
				// Don't accept multipart requests, params may save sent files to
				// disk and we don't want that.
				let url = request.url.clone();
				if let Ok(ref params) = request.get_ref::<Params>() {
					if let params::Value::String(ref text) = params["text"] {
						gui::handle_text_upload(file_sender, text);
						// Redirect to base url
						let mut url: Url = url.into_generic_url();
						url.set_path("");
						Ok(Response::with((status::Found, "Text uploaded successfully",
							Redirect(iron::Url::from_generic_url(url).expect("Cannot parse generated url")))))
					} else {
						Ok(Response::with((status::BadRequest, "Cannot find uploaded text")))
					}
				} else {
					Ok(Response::with((status::BadRequest, "Cannot parse parameters")))
				}
			}
		}
	}
}

fn handle_file(file_sender: Arc<Mutex<FileSender>>, request: &mut Request) -> IronResult<Response> {
	let url = request.url.clone();
	match Multipart::from_request(request) {
		Ok(mut multipart) => {
			while let Ok(Some(entry)) = multipart.read_entry() {
				if entry.name == "File" {
					if let MultipartData::File(multipart_file) = entry.data {
						return handle_file_upload(file_sender, url, multipart_file);
					}
				}
			}
			Ok(Response::with((status::BadRequest, "Uploaded file not found")))
		}
		Err(_) => {
			// No multipart request
			Ok(Response::with((status::Ok,
				"Uploaded file not found (not a multipart request)")))
		}
	}
}

fn handle_file_upload<'a, B: std::io::Read>(file_sender: Arc<Mutex<FileSender>>,
	url: iron::Url, mut multipart_file: MultipartFile<'a, B>) -> IronResult<Response> {
	let (upload_file_name, upload_file_size) = {
		let file_sender = file_sender.lock().unwrap();
		(file_sender.upload_file_name.clone(), file_sender.upload_file_size)
	};
	match multipart_file.save_as_limited(upload_file_name.clone(), upload_file_size) {
		Ok(file) => {
			if file.size >= upload_file_size {
				gui::handle_file_upload(file_sender, file.filename, true);
				Ok(Response::with((status::PayloadTooLarge,
					"The sent file is too large")))
			} else {
				gui::handle_file_upload(file_sender, file.filename, false);
				//
				// Redirect to base url
				let mut url: Url = url.into_generic_url();
				url.set_path("");
				Ok(Response::with((status::Found, "File uploaded successfully",
					Redirect(iron::Url::from_generic_url(url).expect("Cannot parse generated url")))))
			}
		}
		Err(error) => {
			println!("Cannot save file {:?} to {}: {:?}",
					 multipart_file.filename(), upload_file_name, error);
			Ok(Response::with((status::BadRequest, "Cannot save file")))
		}
	}
}

fn handle_file_download(file_sender: Arc<Mutex<FileSender>>, request: &mut Request) -> IronResult<Response> {
	let fs = file_sender.lock().unwrap();
	if let Some(path) = request.url.path().pop().and_then(|i| i.parse::<usize>().ok()).and_then(|index|
		fs.download_files.get(index)
	) {
		let name = get_filename(path);
		println!("Downloaded {}", name);
		let header: Header<ContentDisposition> = Header(ContentDisposition {
			disposition: DispositionType::Attachment,
			parameters: vec![DispositionParam::Filename(
				Charset::Ext(String::from("UTF-8")), None, name.into_bytes(),
			)],
		});
		Ok(Response::with((status::Ok,
			"application/octet-stream".parse::<Mime>().unwrap(),
			header,
			path.clone())))
	} else {
		Ok(Response::with((status::BadRequest, "Invalid index")))
	}
}
