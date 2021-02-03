#[cfg(windows)]
fn main() {
    // Add windows icon inside .exe
    let mut res = winres::WindowsResource::new();
    res.set_icon("mailcatcher.ico");
    res.compile().unwrap();
}

#[cfg(not(windows))]
fn main() {}
