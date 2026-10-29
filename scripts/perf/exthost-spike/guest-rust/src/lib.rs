wit_bindgen::generate!({ path: "../wit", world: "ext" });

struct Ext;

impl Guest for Ext {
    fn activate() {
        codeyard::spike::host::log("activated");
    }

    fn call(input: String) -> String {
        format!("hello {input}")
    }
}

export!(Ext);
