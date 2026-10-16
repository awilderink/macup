use super::*;

#[test]
fn measures_nested_directories_and_caches_subdirectories() {
    let root = crate::selection::tests::fixture_tree();
    let s = Sizer::new(1);
    let docs = root.path().join("Documents");
    let expected =
        ("Documents/a.txt".len() + "Documents/b.txt".len() + "Documents/sub/c.txt".len()) as u64;
    assert_eq!(walk(&s.shared, &docs, 0), expected);
    assert!(
        s.get(&docs.join("sub")).is_some(),
        "subdirectory size was not cached during the parent walk"
    );
}
