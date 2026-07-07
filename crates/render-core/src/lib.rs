pub fn hello() -> &'static str {
    "render-core"
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn smoke() {
        assert_eq!(hello(), "render-core");
    }
}
