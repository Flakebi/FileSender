#![feature(custom_derive, plugin)]
#![plugin(rocket_codegen)]
// Limit for error_chain
#![recursion_limit = "1024"]

#[macro_use]
extern crate clap;
#[macro_use]
extern crate error_chain;
extern crate mime_multipart;
extern crate glib;
extern crate gdk;
extern crate gtk;
#[macro_use]
extern crate lazy_static;
extern crate rocket;
extern crate tempfile;

mod gui;

use std::env;
#[cfg(not(feature = "bundled"))]
use std::fs::{self, File};
#[cfg(not(feature = "bundled"))]
use std::io::{Cursor, Read};
use std::net::{IpAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::thread;
use std::vec::Vec;

use clap::{App, AppSettings, Arg};
use rocket::config;
use rocket::Data;
use rocket::http::ContentType;
use rocket::http::hyper::header;
use rocket::response::{self, Redirect, Responder, Response};
use rocket::request::{Form, FromRequest, Request, Outcome};

mod errors {
	// Create the Error, ErrorKind, ResultExt, and Result types
	error_chain! {
		foreign_links {
			Io(::std::io::Error);
			Multipart(::mime_multipart::Error);
		}
	}
}

use errors::*;

pub struct FileSender {
	upload_file_name: String,
	upload_file_size: usize,
	upload_text: String,
	download_text: String,
	download_files: Vec<PathBuf>,
}

struct Config {
	address: IpAddr,
	port: u16,
	upload_file_name: String,
	upload_file_size: usize,
}

lazy_static! {
	static ref CONFIG: Config = {
		// Parse command line options
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
			.arg(Arg::with_name("upload-filename").short("u")
				.long("upload-filename")
				.default_value("Upload.file")
				.help("The filename that will be used to save uploaded files"))
			.arg(Arg::with_name("upload-size").short("s").long("upload-size")
				.default_value("50000000")
				.validator(validate::<usize>)
				.help("The maximum size for uploaded files"))
			.get_matches();
		Config {
			address: IpAddr::from_str(args.value_of("address").unwrap())
				.unwrap(),
			port: u16::from_str(args.value_of("port").unwrap()).unwrap(),
			upload_file_name: args.value_of("upload-filename").unwrap()
				.to_string(),
			upload_file_size: usize::from_str(args.value_of("upload-size")
				.unwrap()).unwrap(),
		}
	};
	static ref FILE_SENDER: Arc<Mutex<FileSender>> = {
		Arc::new(Mutex::new(FileSender {
			upload_file_name: CONFIG.upload_file_name.clone(),
			upload_file_size: CONFIG.upload_file_size,
			upload_text: String::new(),
			download_text: String::new(),
			download_files: Vec::new(),
		}))
	};
}

fn validate<T: FromStr>(val: String) -> std::result::Result<(), String>
	where T::Err: std::fmt::Display {
	T::from_str(val.as_str()).map(|_| ()).map_err(|e| format!("{}", e))
}

fn get_filename(path: &PathBuf) -> String {
	path.file_name().map(|p| p.to_string_lossy().to_string())
		.unwrap_or(String::from("unknown"))
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
	bundle!(path; "Web/index.html", "Web/static/icon.png",
			"Web/static/style.css",
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
	let server = start_server((CONFIG.address, CONFIG.port));

	gui::main(FILE_SENDER.clone(), &server);

	// Seems like closing a rocket server is not possible, it will be closed
	// when this thread exits, i.e. when the gui window is closed.
}

struct WebFile {
	path: PathBuf,
	content: Vec<u8>,
}

impl WebFile {
	fn new<P: Into<PathBuf>>(path: P) -> Option<WebFile> {
		let path = path.into();
		path.to_str().and_then(|s| get_web_file(s)).map(|content| {
			WebFile {
				path: path,
				content: content,
			}
		})
	}
}

impl<'r> Responder<'r> for WebFile {
	fn respond(self) -> response::Result<'r> {
		let mut response = Response::build();
		self.path.extension().and_then(|e| e.to_str()).map(|e| response.header(
			ContentType::from_extension(e)));
		response.sized_body(Cursor::new(self.content)).ok()
	}
}

fn start_server<To: ToSocketAddrs>(addr: To) -> String {
	// Enable logging
	rocket::logger::init(rocket::LoggingLevel::Normal);
	let addr = addr.to_socket_addrs().unwrap().next().unwrap();
	let config = config::Config::default_for(
			config::Environment::active().unwrap(),
			env::current_dir().unwrap().to_str().unwrap()).unwrap()
		.address(addr.ip().to_string())
		.port(addr.port() as usize);
	let r = rocket::custom(&config).mount("/", routes![
		handle_index,
		handle_static,
		handle_text,
		handle_file_download,
		handle_file_upload,
	]);
	thread::spawn(|| r.launch());
	// Set the port
	addr.to_string()
}

#[get("/")]
fn handle_index() -> Option<String> {
	get_web_string("index.html").map(|mut content| {
		let fs = FILE_SENDER.lock().unwrap();
		content = content.replace("{download_text}", fs.download_text.as_str());
		content = content.replace("{upload_text}", fs.upload_text.as_str());
		let text = fs.download_files.iter().map(|p| get_filename(p))
			.enumerate().fold(String::new(), |s, (i, p)|
			format!(r###"{}<li><a href="data/download/{i}">{path}</a></li>"###,
				s, i = i, path = p));
		content = content.replace("{download_files}",
			format!(r###"<ul class="file-list">{}</ul>"###, text).as_str());
		content
	})
}

#[get("/static/<file..>")]
fn handle_static<'r>(file: PathBuf) -> Option<WebFile> {
	WebFile::new(file)
}

#[derive(FromForm)]
struct Text { text: String }

#[post("/data/text", data = "<text>")]
fn handle_text(text: Form<Text>) -> Redirect {
	gui::handle_text_upload(FILE_SENDER.clone(), &text.get().text);
	// Redirect to base url
	Redirect::to("/")
}

#[get("data/download/<index>")]
fn handle_file_download<'r>(index: usize)
	-> Option<Result<Response<'r>>> {
	let fs = FILE_SENDER.lock().unwrap();
	if let Some(path) = fs.download_files.get(index) {
		let name = get_filename(path);
		println!("Downloaded {}", name);
		let header = header::ContentDisposition {
			disposition: header::DispositionType::Attachment,
			parameters: vec![header::DispositionParam::Filename(
				header::Charset::Ext(String::from("UTF-8")), None,
				name.into_bytes(),
			)],
		};
		Some(File::open(path).map(|f| {
			Response::build()
				.header(ContentType::new("application", "octet-stream"))
				.header(header)
				.streamed_body(f)
				.finalize()
		}).map_err(|e| e.into()))
	} else {
		None
	}
}

/// Convert `rocket` headers back to `hyper` headers.
struct MyHeaders(header::Headers);

impl<'a, 'r> FromRequest<'a, 'r> for MyHeaders {
	type Error = Error;
	fn from_request(request: &'a Request<'r>)
		-> Outcome<Self, Self::Error> {
		let mut headers = header::Headers::new();
		let rh = request.headers();
		for name in rh.iter().map(|h| h.name)
			.fold(Vec::<String>::new(), |mut v, n| {
				// Take each name only once
				if v.last().map(|l| *l != n.as_str()).unwrap_or(true) {
					v.push(n.to_string());
				}
				v
			}) {
			let values = rh.get(&name).map(|v| v.as_bytes().to_vec()).collect();
			headers.set_raw(name, values);
		}
		rocket::Outcome::Success(MyHeaders(headers))
	}
}

#[post("data/upload", data = "<data>")]
fn handle_file_upload(data: Data, headers: MyHeaders) -> Result<Redirect> {
	let (upload_file_name, upload_file_size) = {
		let fs = FILE_SENDER.lock().unwrap();
		(fs.upload_file_name.clone(), fs.upload_file_size)
	};
	// Read the form data with a limited size
	let mut reader = data.open().take(upload_file_size as u64);

	let mut nodes = mime_multipart::read_multipart_body(&mut reader, &headers.0, false)?;
	if let Some(&mut mime_multipart::Node::File(ref mut file)) = nodes.first_mut() {
		if file.size.unwrap() > upload_file_size {
			bail!("File too large");
		}
		let mut name = file.filename()?.unwrap_or(upload_file_name.clone());
		// Check if all characters are in a whitelist
		if !name.chars().all(|c| "abcdefghijklmnopqrstuvwxyz\
			ABCDEFGHIJKLMNOPQRSTUVWXYZ\
			0123456789.-_".contains(c)) {
			name = upload_file_name;
		}
		// Check if the file exists
		let mut i = 0;
		let mut dest = Path::new(&name).to_path_buf();
		while dest.exists() {
			dest = format!("{}-{}", i, name).into();
			i += 1;
		}
		println!("Moving uploaded file {:?} to {:?}", file.path, dest);
		// Move the file
		fs::copy(&file.path, &dest)?;
		gui::handle_file_upload(FILE_SENDER.clone(), Some(dest.into_os_string().into_string().unwrap()), false);
	} else {
		bail!("Uploaded file not found");
	}

	// Redirect to base url
	Ok(Redirect::to("/"))
}
