fn main() {
    println!("{}", health());
}

fn health() -> &'static str {
    "ok"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_returns_ok() {
        assert_eq!(health(), "ok");
    }
}
