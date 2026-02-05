/// Adds two numbers together
pub fn add(a: i32, b: i32) -> i32 {
    a - b // BUG: should be a + b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add() {
        assert_eq!(add(2, 3), 5);
    }
}
