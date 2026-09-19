// Artist name sorting: the leading-article rule shared by the artist
// index (bucketing and order) and ArtistID3.sortName.

// Leading articles the index strips before bucketing (Navidrome's default
// list). Serve the same string in ignoredArticles so clients know the rule.
pub const IGNORED_ARTICLES: &str = "The El La Los Las Le Les";

// The name split at its leading article: ("Beatles", Some("The")) for
// "The Beatles"; a bare article yields ("", Some(article)). Matching is
// case-insensitive; the pieces keep the original spelling.
fn split_article(name: &str) -> (&str, Option<&str>) {
    let lower = name.to_lowercase();
    for article in IGNORED_ARTICLES.split(' ') {
        let a = article.to_lowercase();
        if lower == a {
            return ("", Some(name));
        }
        if lower.starts_with(&format!("{a} ")) {
            let (head, rest) = name.split_at(article.len());
            return (rest.trim_start(), Some(head));
        }
    }
    (name, None)
}

// The index key: the name without a leading article, lowercased.
// "The Beatles" sorts as "beatles"; a bare article sorts as "".
pub fn sort_key(name: &str) -> String {
    split_article(name).0.to_lowercase()
}

// The sortName clients display and sort by: the leading article moves
// to the end, MusicBrainz-style ("Beatles, The"). Names without an
// article come back unchanged.
pub fn sort_name(name: &str) -> String {
    match split_article(name) {
        (rest, Some(article)) if !rest.is_empty() => format!("{rest}, {article}"),
        _ => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_key_strips_leading_article() {
        assert_eq!(sort_key("The Beatles"), "beatles");
        assert_eq!(sort_key("Los Lobos"), "lobos");
        assert_eq!(sort_key("Theory of a Deadman"), "theory of a deadman");
        assert_eq!(sort_key("The"), "");
    }

    #[test]
    fn sort_name_moves_article_to_the_end() {
        assert_eq!(sort_name("The Beatles"), "Beatles, The");
        assert_eq!(sort_name("the strokes"), "strokes, the");
        assert_eq!(sort_name("Four Year Strong"), "Four Year Strong");
        assert_eq!(sort_name("The"), "The");
    }
}
