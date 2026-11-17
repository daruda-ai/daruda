use super::*;

#[test]
fn line_endings_do_not_make_a_description_differ() {
    // (label, a, b, same)
    let cases = [
        ("identical", "a\nb", "a\nb", true),
        ("CRLF from GitHub", "a\r\nb", "a\nb", true),
        ("an edit", "a\nb", "a\nc", false),
        ("trailing text", "a", "a\n", false),
    ];
    for (label, a, b, same) in cases {
        assert_eq!(same_text(a, b), same, "{label}");
    }
}
