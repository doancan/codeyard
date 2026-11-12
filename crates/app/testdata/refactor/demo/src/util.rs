pub fn hesapla(a: i32, b: i32) -> i32 {
    a + b
}

pub fn iki_kat(x: i32) -> i32 {
    hesapla(x, x) // 🦀🦀 hesapla çağrısı
}
