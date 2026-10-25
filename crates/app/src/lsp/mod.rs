//! Language server client. The server runs as the agent's piped process
//! on the machine where the files live (local or SSH). `transport` is the JSON-RPC side (framing,
//! request/response matching, replies to the server's own requests), `parse` turns responses into
//! editor types, `servers` is the table of known servers; all of it is re-exported here.
//! Independent of the UI; blocking calls are made on background threads.

mod parse;
mod servers;
mod transport;

pub use parse::*;
pub use servers::*;
pub use transport::*;

/// (line, UTF-16 column) range.
pub type Range16 = ((u32, u32), (u32, u32));

// --- URI ---

pub fn path_to_uri(path: &str) -> String {
    schema::file_uri::from_path(path)
}

pub fn uri_to_path(uri: &str) -> Option<String> {
    schema::file_uri::to_path(uri)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_file_uris_round_trip_drive_and_unc_paths() {
        let path = "C:/şablon boşluk/src/main.rs";
        let uri = path_to_uri(path);
        assert_eq!(uri, "file:///C:/%C5%9Fablon%20bo%C5%9Fluk/src/main.rs");
        assert_eq!(path_to_uri(r"C:\şablon boşluk\src\main.rs"), uri);
        assert_eq!(uri_to_path(&uri).as_deref(), Some(path));
        assert_eq!(uri_to_path("file:///C%3A/work/main.rs").as_deref(), Some("C:/work/main.rs"));
        assert_eq!(path_to_uri(r"\\server\share\a b.rs"), "file://server/share/a%20b.rs");
        assert_eq!(uri_to_path("file://server/share/a%20b.rs").as_deref(), Some("//server/share/a b.rs"));
        assert_eq!(uri_to_path("file://localhost/home/ada/main.rs").as_deref(), Some("/home/ada/main.rs"));
    }

    #[test]
    fn uris_round_trip() {
        let p = "/home/kullanıcı/my proje/a#b.rs";
        let u = path_to_uri(p);
        assert!(u.starts_with("file:///home/kullan%C4%B1c%C4%B1/my%20proje/a%23b.rs"), "{u}");
        assert_eq!(uri_to_path(&u).as_deref(), Some(p));
        assert_eq!(uri_to_path("file:///a/b%20c").as_deref(), Some("/a/b c"));
    }
}
