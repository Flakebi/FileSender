use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use iron;
use gdk::enums::key;
use glib;
use gtk;
use gtk::prelude::*;
use gtk::Builder;

use ::FileSender;

thread_local!(static GLOBAL: RefCell<Option<Gui>> = RefCell::new(None));

struct Gui {
	builder: Builder,
}

pub fn main(file_sender: Arc<Mutex<FileSender>>, server: &iron::Listening) {
	// Initialize gui
	gtk::init().expect("Failed to initialize GTK");

	// Create window
	let builder = Builder::new_from_string(::get_string("Window.glade").unwrap().as_str());
	// Create Window
	let window: gtk::Window = builder.get_object("window").unwrap();
	let quit_button: gtk::Button = builder.get_object("quitButton").unwrap();
	quit_button.connect_clicked(move |_| {
		gtk::main_quit();
	});

	let description_label: gtk::Entry = builder.get_object("descriptionLabel").unwrap();
	description_label.set_text(format!("{}", server.socket).as_str());
	// Select the text
	//description_label.select_region(0, -1);

	let fs = file_sender.clone();
	let text_field: gtk::TextView = builder.get_object("downloadTextField").unwrap();
	text_field.get_buffer().unwrap().connect_changed(move |buffer| {
		fs.lock().unwrap().download_text = buffer.get_text(&buffer.get_start_iter(), &buffer.get_end_iter(), false).unwrap();
	});
	let fs = file_sender.clone();
	let text_field: gtk::TextView = builder.get_object("uploadTextField").unwrap();
	text_field.get_buffer().unwrap().connect_changed(move |buffer| {
		fs.lock().unwrap().upload_text = buffer.get_text(&buffer.get_start_iter(), &buffer.get_end_iter(), false).unwrap();
	});
	let download_files: gtk::ListBox = builder.get_object("downloadList").unwrap();
	download_files.connect_button_press_event(|_, event| {
		// Check if the right mouse button was clicked
		let button = event.as_ref().button;
		if button == 3 {
			GLOBAL.with(|global| {
				let menu: gtk::Menu = global.borrow().as_ref().unwrap().builder.get_object("downloadFileMenu").unwrap();
				menu.popup_easy(button, event.get_time());
			});
		}
		Inhibit(false)
	});
	let fs = file_sender.clone();
	let add_button: gtk::Button = builder.get_object("addDownloadFile").unwrap();
	add_button.connect_clicked(move |_| {
		GLOBAL.with(|global| {
			let dialog: gtk::FileChooserDialog = global.borrow().as_ref().unwrap()
				.builder.get_object("downloadFileChooser").unwrap();
			if dialog.run() == gtk::ResponseType::Accept.into() {
				handle_file_add(fs.clone(), dialog.get_filenames());
			}
			dialog.hide();
		});
	});
	let fs = file_sender.clone();
	let delete_button: gtk::MenuItem = builder.get_object("deleteDownloadFileMenuItem").unwrap();
	delete_button.connect_activate(move |_| {
		handle_file_delete(fs.clone());
	});
	let fs = file_sender.clone();
	let list: gtk::ListBox = builder.get_object("downloadList").unwrap();
	list.connect_key_press_event(move |_, key| {
		if key.get_keyval() == key::Delete {
			handle_file_delete(fs.clone());
		}
		Inhibit(false)
	});

	window.connect_delete_event(|widget, _| {
		// Close the application
		widget.hide();
		gtk::main_quit();
		Inhibit(true)
	});

	GLOBAL.with(|global| {
		let gui = Gui {
			builder: builder,
		};
		*global.borrow_mut() = Some(gui);
	});

	window.show_all();

	// Main loop
	gtk::main();

	window.destroy();
	// Remove Gui object from thread local storage
	GLOBAL.with(move |global| {
		*global.borrow_mut() = None;
	});
}

pub fn handle_text_upload(file_sender: Arc<Mutex<FileSender>>, text: &str) {
	let text = text.to_string();
	glib::idle_add(move || {
		GLOBAL.with(|global| {
			let field: gtk::TextView = global.borrow_mut().as_mut().unwrap().builder.get_object("uploadTextField").unwrap();
			field.get_buffer().expect("The TextView should have a buffer").set_text(text.as_str());
		});

		let mut fs = file_sender.lock().unwrap();
		fs.upload_text = text.clone();

		Continue(false)
	});
}

pub fn handle_file_upload(_: Arc<Mutex<FileSender>>, filename: Option<String>, truncated: bool) {
	glib::idle_add(move || {
		GLOBAL.with(|global| {
			let field: gtk::Entry = global.borrow_mut().as_mut().unwrap().builder.get_object("uploadFileLabel").unwrap();
			if truncated {
				println!("The file was truncated");
				field.set_text("Truncated");
			} else {
				field.set_text(filename.as_ref().map(|s| s.as_str()).unwrap_or("unknown file"));
			}
		});
		Continue(false)
	});
}

fn handle_file_add(file_sender: Arc<Mutex<FileSender>>, filenames: Vec<PathBuf>) {
	file_sender.lock().unwrap().download_files.append(&mut filenames.clone());
	GLOBAL.with(|global| {
		let list: gtk::ListBox = global.borrow_mut().as_mut().unwrap().builder.get_object("downloadList").unwrap();
		for path in filenames {
			let label = gtk::Label::new(Some(::get_filename(&path).as_str()));
			label.show_all();
			list.insert(&label, -1);
		}
	});
}

fn handle_file_delete(file_sender: Arc<Mutex<FileSender>>) {
	GLOBAL.with(|global| {
		let list: gtk::ListBox = global.borrow_mut().as_mut().unwrap().builder.get_object("downloadList").unwrap();
		if let Some(row) = list.get_selected_row() {
			file_sender.lock().unwrap().download_files.remove(row.get_index() as usize);
			list.remove(&row);
		}
	});
}
