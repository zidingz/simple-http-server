#[macro_use]
extern crate clap;
extern crate pretty_bytes;
extern crate chrono;
extern crate ansi_term;
extern crate iron;
extern crate multipart;

use std::env;
use std::str::FromStr;
use std::net::IpAddr;
use std::fs::{self, File};
use std::path::{PathBuf};
use std::error::Error;
use std::time::{SystemTime, UNIX_EPOCH};
use std::os::unix::ffi::OsStrExt;

use iron::headers;
use iron::status;
use iron::mime;
use iron::method;
use iron::modifiers::Redirect;
use iron::{Iron, Request, Response, IronResult, Set, Chain, Handler,
           AfterMiddleware};
use multipart::server::{Multipart, SaveResult};
use pretty_bytes::converter::convert;
use chrono::{DateTime, Local, TimeZone};
use ansi_term::Colour::{Red, Green, Yellow, Blue};

const ROOT_LINK: &'static str = "<a href=\"/\">[ROOT]</a>";

fn main() {
    let matches = clap::App::new("Simple HTTP Server")
        .version(crate_version!())
        .arg(clap::Arg::with_name("root")
             .index(1)
             .validator(|s| {
                 match File::open(s) {
                     Ok(f) => {
                         if f.metadata().unwrap().is_dir() { Ok(()) } else {
                             Err("Not directory".to_owned())
                         }
                     },
                     Err(e) => Err(e.description().to_string())
                 }
             })
             .help("Root directory"))
        .arg(clap::Arg::with_name("index")
             .short("i")
             .long("index")
             .help("Enable automatic render index page [index.html, index.htm]"))
        .arg(clap::Arg::with_name("upload")
             .short("u")
             .long("upload")
             .help("Enable upload files (multiple select)"))
        .arg(clap::Arg::with_name("ip")
             .long("ip")
             .takes_value(true)
             .default_value("0.0.0.0")
             .validator(|s| {
                 match IpAddr::from_str(&s) {
                     Ok(_) => Ok(()),
                     Err(e) => Err(e.description().to_string())
                 }
             })
             .help("IP address to bind"))
        .arg(clap::Arg::with_name("port")
             .short("p")
             .long("port")
             .takes_value(true)
             .default_value("8000")
             .validator(|s| {
                 match s.parse::<u16>() {
                     Ok(_) => Ok(()),
                     Err(e) => Err(e.description().to_string())
                 }
             })
             .help("Port number"))
        .arg(clap::Arg::with_name("threads")
             .short("t")
             .long("threads")
             .takes_value(true)
             .default_value("3")
             .validator(|s| {
                 match s.parse::<u8>() {
                     Ok(v) => {
                         if v > 0 { Ok(()) } else {
                             Err("Nagetive threads".to_owned())
                         }
                     }
                     Err(e) => Err(e.description().to_string())
                 }
             })
             .help("How many worker threads"))
        .get_matches();
    let root = matches
        .value_of("root")
        .map(|s| PathBuf::from(s))
        .unwrap_or(env::current_dir().unwrap());
    let index = matches.is_present("index");
    let upload = matches.is_present("upload");
    let ip = matches.value_of("ip").unwrap();
    let port = matches
        .value_of("port")
        .unwrap()
        .parse::<u16>()
        .unwrap();
    let threads = matches
        .value_of("threads")
        .unwrap()
        .parse::<u8>()
        .unwrap();

    let addr = format!("{}:{}", ip, port);
    println!("  Index: {}, Upload: {}, Threads: {}",
             Blue.paint(index.to_string()),
             Blue.paint(upload.to_string()),
             Blue.paint(threads.to_string()));
    println!("   Root: {}", Blue.paint(root.to_str().unwrap()));
    println!("Address: {}", Blue.paint(format!("http://{}", addr)));
    println!("======== [{}] ========", Blue.paint(now_string()));

    let mut chain = Chain::new(MainHandler{root, index, upload});
    chain.link_after(RequestLogger);
    let mut server = Iron::new(chain);
    server.threads = threads as usize;
    server.http(&addr).expect(format!("Could not bind on: {}", addr).as_str());
}

struct MainHandler { root: PathBuf, index: bool, upload: bool }
struct RequestLogger;

impl MainHandler {

    fn error(&self, s: status::Status, msg: &str)  -> IronResult<Response> {
        let mut resp = Response::with((s, format!(
            "<html><body>{root_link} <hr /><div>[<strong style=\"color:red;\">ERROR {code}</strong>]: {msg}</div></body></html>",
            root_link=ROOT_LINK,
            code=s.to_u16(),
            msg=msg
        )));
        resp.headers.set(headers::ContentType::html());
        Ok(resp)
    }

    fn save_files(&self, req: &mut Request, path: &PathBuf) -> Result<(), (status::Status, String)> {
        match Multipart::from_request(req) {
            Ok(mut multipart) => {
                // Fetching all data and processing it.
                // save().temp() reads the request fully, parsing all fields and saving all files
                // in a new temporary directory under the OS temporary directory.
                match multipart.save().temp() {
                    SaveResult::Full(entries) => {
                        for (_, files) in entries.files {
                            for file in files {
                                let mut target_path = path.clone();
                                target_path.push(file.filename.unwrap());
                                if let Err(errno) = fs::copy(file.path, target_path) {
                                    return Err((status::InternalServerError, format!("Copy file failed: {}", errno)));
                                }
                            }
                        }
                        Ok(())
                    },
                    SaveResult::Partial(_entries, reason) => {
                        Err((status::InternalServerError, reason.unwrap_err().description().to_owned()))
                    }
                    SaveResult::Error(error) => Err((status::InternalServerError, error.description().to_owned())),
                }
            }
            Err(_) => Err((status::BadRequest ,"The request is not multipart".to_owned()))
        }
    }

    fn send_file(&self, mut resp: Response, path: &PathBuf) -> IronResult<Response> {
        resp.set_mut(path.as_path());
        if resp.headers.get::<headers::ContentType>() == Some(
            &headers::ContentType(mime::Mime(mime::TopLevel::Text, mime::SubLevel::Plain, vec![]))
        ) {
            resp.headers.set(headers::ContentDisposition {
                disposition: headers::DispositionType::Attachment,
                parameters: vec![headers::DispositionParam::Filename(
                    headers::Charset::Ext("utf-8".to_owned()), // The character set for the bytes of the filename
                    None, // The optional language tag (see `language-tag` crate)
                    path.file_name().unwrap().as_bytes().to_vec() // the actual bytes of the filename
                )]
            });
            let default_mime: iron::mime::Mime = "application/octet-stream".parse().unwrap();
            resp.headers.set(headers::ContentType(default_mime));
        }
        return Ok(resp)
    }
}

impl Handler for MainHandler {
    fn handle(&self, req: &mut Request) -> IronResult<Response> {
        let mut path = self.root.clone();
        for part in req.url.path() {
            path.push(part);
        }

        if self.upload && req.method == method::Post {
            if let Err((s, msg)) = self.save_files(req, &path) {
                return self.error(s, &msg);
            } else {
                return Ok(Response::with((status::Found, Redirect(req.url.clone()))))
            }
        }

        match File::open(&path) {
            Ok(f) => {
                let mut resp = Response::with(status::Ok);
                let metadata = f.metadata().unwrap();
                if metadata.is_dir() {
                    let mut rows = Vec::new();

                    let path_prefix = req.url.path()
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<&str>>();
                    let breadcrumb = if path_prefix.len() > 0 {
                        let mut breadcrumb = path_prefix.clone();
                        let mut bread_links: Vec<String> = Vec::new();
                        bread_links.push(breadcrumb.pop().unwrap().to_owned());
                        while breadcrumb.len() > 0 {
                            let link = breadcrumb.join("/");
                            bread_links.push(format!(
                                "<a href=\"/{link}\">{label}</a>",
                                link=link, label=breadcrumb.pop().unwrap().to_owned(),
                            ));
                        }
                        bread_links.push(ROOT_LINK.to_owned());
                        bread_links.reverse();
                        bread_links.join(" / ")
                    } else { ROOT_LINK.to_owned() };

                    if path_prefix.len() > 0 {
                        let mut link = path_prefix.clone();
                        link.pop();
                        rows.push(format!(
                            "<tr><td><a href=\"/{link}\"><strong>{label}</strong></a></td> <td></td> <td></td></tr>",
                            link=link.join("/"), label="[Parent Directory]"
                        ));
                    } else {
                        rows.push("<tr><td>&nbsp;</td></tr>".to_owned());
                    }
                    for entry in fs::read_dir(&path).unwrap() {
                        let entry = entry.unwrap();
                        let entry_meta = entry.metadata().unwrap();
                        let file_name = entry.file_name().into_string().unwrap();
                        if self.index {
                            for fname in vec!["index.html", "index.htm"] {
                                if file_name == fname {
                                    path.push(file_name);
                                    return self.send_file(resp, &path);
                                }
                            }
                        }
                        let file_modified = system_time_to_date_time(entry_meta.modified().unwrap())
                            .format("%Y-%m-%d %H:%M:%S").to_string();
                        let file_size = convert(entry_meta.len() as f64);
                        let file_type = entry_meta.file_type();
                        let link_style = if file_type.is_dir() {
                            "style=\"text-decoration: none; font-weight: bold;\"".to_owned()
                        } else {
                            "style=\"text-decoration: none;\"".to_owned()
                        };
                        let mut link = path_prefix.clone();
                        link.push(&file_name);
                        let link = link.join("/");
                        rows.push(format!(
                            "<tr><td><a {} href=\"/{}\">{}</a></td> <td style=\"color:#888;\">[{}]</td> <td><bold>{}</bold></td></tr>",
                            link_style, link, file_name, file_modified, file_size
                        ));
                    }
                    resp.headers.set(headers::ContentType::html());
                    let upload_form = if self.upload {
                        format!(r#"
<form style="margin-top: 1em;" action="/{path}" method="POST" enctype="multipart/form-data">
  <input type="file" name="files" accept="*" multiple />
  <input type="submit" value="Upload" />
</form>
"#,
                                path=path_prefix.join("/"))
                    } else { "".to_owned() };
                    resp.set_mut(format!(
                        r#"
<html>
<body>
  {upload_form}
  <div>{breadcrumb}</div>
  <hr />
  <table>{rows}</table>
</body>
</html>"#,
                        upload_form=upload_form,
                        breadcrumb=breadcrumb,
                        rows=rows.join("\n")));
                    Ok(resp)
                } else {
                    self.send_file(resp, &path)
                }
            },
            Err(e) => self.error(status::NotFound, e.description().to_string().as_str())
        }
    }
}

impl AfterMiddleware for RequestLogger {
    fn after(&self, req: &mut Request, resp: Response) -> IronResult<Response> {
        let status = resp.status.unwrap();
        let status_str = if status.is_success() {
            Green.bold().paint(status.to_u16().to_string())
        } else if status.is_informational() || status.is_redirection() {
            Yellow.bold().paint(status.to_u16().to_string())
        } else {
            Red.bold().paint(status.to_u16().to_string())
        };

        println!(
            // datetime, remote-ip, status-code, method, url-path
            "[{}] - {} - {} - {} /{}",
            now_string(),
            req.remote_addr.ip(),
            status_str,
            req.method,
            req.url.as_ref().to_string().splitn(4, '/').collect::<Vec<&str>>().pop().unwrap()
        );
        Ok(resp)
    }
}

fn now_string() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

fn system_time_to_date_time(t: SystemTime) -> DateTime<Local> {
    let (sec, nsec) = match t.duration_since(UNIX_EPOCH) {
        Ok(dur) => (dur.as_secs() as i64, dur.subsec_nanos()),
        Err(e) => { // unlikely but should be handled
            let dur = e.duration();
            let (sec, nsec) = (dur.as_secs() as i64, dur.subsec_nanos());
            if nsec == 0 {
                (-sec, 0)
            } else {
                (-sec - 1, 1_000_000_000 - nsec)
            }
        },
    };
    Local.timestamp(sec, nsec)
}
