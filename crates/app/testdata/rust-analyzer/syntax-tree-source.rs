fn main() {
    println!("{}", demo::add(2, 3));
}

#[cfg(test)]
mod tests {
    #[test]
    fn main_is_fine() {}
}
