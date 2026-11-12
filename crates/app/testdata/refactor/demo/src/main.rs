mod util;
mod ayar;
mod rapor;

fn main() {
    let sonuç = util::hesapla(2, 3);
    println!("🦀 sonuç = {}", sonuç);
    println!("{}", ayar::çarpan_ile(util::hesapla(1, 1)));
    println!("{}", rapor::özet());
}
