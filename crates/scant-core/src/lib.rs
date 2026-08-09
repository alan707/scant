pub fn greeting() -> &'static str {
    "scant"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greeting_is_scant() {
        assert_eq!(greeting(), "scant");
    }
}
