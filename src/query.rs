//! Typed query layer: pagination, sorting, filtering and `include` parsing for
//! JSON:API list/show endpoints.
//!
//! Everything here is pure parsing/data-shaping logic with no actix
//! dependency; it operates on raw query strings via [`serde_qs`] so it can be
//! exercised in unit tests (and reused by any transport, not just actix-web).

use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde::de::IntoDeserializer;
use serde_derive::{Deserialize, Serialize};

use crate::document::PaginationLinks;
use crate::error::Error;

/// Cursor-pagination request parameters, nested under `page[...]` in the
/// query string (e.g. `page[size]=25&page[after]=abc123`).
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
pub struct PageParams {
    /// Maximum number of items to return.
    pub size: Option<u32>,
    /// Opaque cursor: return items after this position.
    pub after: Option<String>,
    /// Opaque cursor: return items before this position.
    pub before: Option<String>,
}

/// Sort direction for a single sort key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Ascending order (the default, no prefix in the `sort` param).
    Asc,
    /// Descending order (`-` prefix in the `sort` param).
    Desc,
}

/// Ordered list of sort keys parsed from the JSON:API `sort` query parameter,
/// e.g. `sort=-created_at,name` parses (with `K = MySort`) into
/// `SortSpec(vec![(MySort::CreatedAt, Direction::Desc), (MySort::Name, Direction::Asc)])`.
///
/// `K` is typically a user-defined enum deriving `Deserialize` (commonly
/// `#[serde(rename_all = "snake_case")]`); a segment that doesn't match any
/// variant of `K` is a deserialize error (which handlers turn into a 400).
#[derive(Debug, Clone, PartialEq)]
pub struct SortSpec<K>(pub Vec<(K, Direction)>);

impl<K> Default for SortSpec<K> {
    fn default() -> Self {
        SortSpec(Vec::new())
    }
}

impl<K> SortSpec<K> {
    /// True if no sort keys were specified.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Iterate over the parsed `(key, direction)` pairs in order.
    pub fn iter(&self) -> std::slice::Iter<'_, (K, Direction)> {
        self.0.iter()
    }
}

impl<K> IntoIterator for SortSpec<K> {
    type Item = (K, Direction);
    type IntoIter = std::vec::IntoIter<(K, Direction)>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'de, K> serde::Deserialize<'de> for SortSpec<K>
where
    K: serde::Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        let mut out = Vec::new();
        for segment in raw.split(',') {
            if segment.is_empty() {
                continue;
            }
            let (direction, key_str) = match segment.strip_prefix('-') {
                Some(rest) => (Direction::Desc, rest),
                None => (Direction::Asc, segment),
            };
            let key = K::deserialize(key_str.to_owned().into_deserializer())
                .map_err(|err: serde::de::value::Error| serde::de::Error::custom(err.to_string()))?;
            out.push((key, direction));
        }
        Ok(SortSpec(out))
    }
}

/// Marker type for resources that don't support sorting at all. Deserializing
/// it always errors — meant to be used as `SortSpec`'s `K` is not applicable;
/// instead this stands in directly for the `sort` field's value type on
/// resources without sort support. If `sort` is absent from the query string,
/// `Default` is used and this is never deserialized; if it's present, the
/// request is rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Unsorted;

impl<'de> serde::Deserialize<'de> for Unsorted {
    fn deserialize<D>(_deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Err(serde::de::Error::custom(
            "this resource does not support sorting",
        ))
    }
}

/// A single dotted `include` path, e.g. `"comments.author"` parses into
/// `["comments", "author"]`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct IncludePath(pub Vec<String>);

/// The full set of requested `include` paths, parsed from a comma-separated
/// `include` query parameter, e.g. `include=author,comments.author`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct IncludeSet(pub Vec<IncludePath>);

impl IncludeSet {
    /// True if no `include` paths were requested.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// True if any requested path's first segment matches `name`, i.e. the
    /// named relationship was requested for inclusion (whether directly or
    /// as the root of a deeper path).
    pub fn contains(&self, name: &str) -> bool {
        self.0
            .iter()
            .any(|path| path.0.first().map(|s| s.as_str()) == Some(name))
    }

    /// Iterate over the requested paths.
    pub fn iter(&self) -> std::slice::Iter<'_, IncludePath> {
        self.0.iter()
    }
}

impl<'de> serde::Deserialize<'de> for IncludeSet {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        let paths = raw
            .split(',')
            .filter(|segment| !segment.is_empty())
            .map(|segment| IncludePath(segment.split('.').map(str::to_owned).collect()))
            .collect();
        Ok(IncludeSet(paths))
    }
}

/// A string filter operator: `filter[name][eq]=x` or `filter[name][contains]=y`.
///
/// Externally tagged (the single key of the nested map selects the variant).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StringMatch {
    /// Exact match.
    Eq(String),
    /// Substring match.
    Contains(String),
}

/// A numeric/orderable range filter: `filter[price][gte]=10&filter[price][lte]=20`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default, bound(deserialize = "T: serde::Deserialize<'de>"))]
pub struct Range<T> {
    /// Exact value.
    pub eq: Option<T>,
    /// Strictly greater than.
    pub gt: Option<T>,
    /// Greater than or equal to.
    pub gte: Option<T>,
    /// Strictly less than.
    pub lt: Option<T>,
    /// Less than or equal to.
    pub lte: Option<T>,
}

impl<T> Default for Range<T> {
    fn default() -> Self {
        Range {
            eq: None,
            gt: None,
            gte: None,
            lt: None,
            lte: None,
        }
    }
}

impl<T> Range<T> {
    /// True if no bound was specified.
    pub fn is_empty(&self) -> bool {
        self.eq.is_none() && self.gt.is_none() && self.gte.is_none() && self.lt.is_none() && self.lte.is_none()
    }
}

/// Full typed query for a `GET /{type}/` list endpoint: filters, pagination,
/// sort and `include`, all parsed together from one query string by
/// [`parse_query`].
#[derive(Debug, Clone, Deserialize)]
#[serde(default, bound(deserialize = "F: serde::Deserialize<'de> + Default, K: serde::Deserialize<'de>"))]
pub struct ListQuery<F, K> {
    /// Resource-specific filter, nested under `filter[...]`.
    pub filter: F,
    /// Cursor pagination params, nested under `page[...]`.
    pub page: PageParams,
    /// Parsed `sort` param.
    pub sort: SortSpec<K>,
    /// Parsed `include` param.
    pub include: IncludeSet,
}

impl<F: Default, K> Default for ListQuery<F, K> {
    fn default() -> Self {
        ListQuery {
            filter: F::default(),
            page: PageParams::default(),
            sort: SortSpec::default(),
            include: IncludeSet::default(),
        }
    }
}

/// Full typed query for a `GET /{type}/{id}` show endpoint: just `include`.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
pub struct ShowQuery {
    /// Parsed `include` param.
    pub include: IncludeSet,
}

/// Parse a raw query string (as found after the `?` in a request URI) into a
/// typed query struct (typically [`ListQuery`] or [`ShowQuery`]), using
/// `serde_qs` in form-encoding-tolerant mode with a nesting depth of 5.
///
/// Any parse failure (unknown sort key, malformed nesting, etc.) is mapped to
/// a `400 Bad Request` [`Error`] with the underlying `serde_qs` error message
/// attached as `detail`.
pub fn parse_query<'de, Q: serde::Deserialize<'de>>(query_string: &'de str) -> Result<Q, Error> {
    serde_qs::Config::new()
        .max_depth(5)
        .deserialize_str(query_string)
        .map_err(|err| {
            let mut e = Error::new_bad_request("invalid query string");
            e.detail = Some(err.to_string());
            e
        })
}

/// Whether a [`CursorPage`]'s total item count is exact or a best-guess
/// estimate (e.g. from a database query planner).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Total {
    /// An exact count.
    Exact(usize),
    /// A best-guess estimate.
    Estimated(usize),
}

/// A page of results plus what's needed to build pagination links/meta.
///
/// Built via the "query `LIMIT size+1`" idiom: [`CursorPage::from_probe`]
/// takes up to `size + 1` rows and derives `has_more` from whether the extra
/// row was present.
#[derive(Debug, Clone, PartialEq)]
pub struct CursorPage<T> {
    /// The page of items (already truncated to the requested page size).
    pub items: Vec<T>,
    /// True if there are more items beyond this page.
    pub has_more: bool,
    /// Optional total item count across all pages.
    pub total: Option<Total>,
}

impl<T> CursorPage<T> {
    /// Build a page from a probe query that fetched up to `page_size + 1`
    /// rows: if more than `page_size` rows came back, truncate to
    /// `page_size` and set `has_more = true`.
    pub fn from_probe(mut rows: Vec<T>, page_size: usize) -> Self {
        let has_more = rows.len() > page_size;
        if has_more {
            rows.truncate(page_size);
        }
        CursorPage {
            items: rows,
            has_more,
            total: None,
        }
    }

    /// Map each item, preserving `has_more`/`total`.
    pub fn map<U>(self, mut f: impl FnMut(T) -> U) -> CursorPage<U> {
        CursorPage {
            items: self.items.into_iter().map(&mut f).collect(),
            has_more: self.has_more,
            total: self.total,
        }
    }

    /// Attach a total item count.
    pub fn with_total(mut self, t: Total) -> Self {
        self.total = Some(t);
        self
    }

    /// An empty page with no more results.
    pub fn empty() -> Self {
        CursorPage {
            items: Vec::new(),
            has_more: false,
            total: None,
        }
    }
}

/// Percent-encoding set used for cursor values appended to pagination links:
/// escape everything but alphanumerics, so `+`, `/`, `=` (common in
/// base64-ish cursors) and JSON:API delimiter characters are all encoded.
const CURSOR_ENCODE_SET: &AsciiSet = NON_ALPHANUMERIC;

/// Rebuild `{path}?{query}`, keeping every existing query pair except any
/// whose *decoded* key is `page[after]` or `page[before]` (tolerating both
/// literal and percent-encoded bracket forms, e.g. `page%5Bafter%5D`), then
/// appending `page[after]=<after_cursor>` with the cursor value
/// percent-encoded. Original ordering of retained pairs is preserved.
pub fn next_page_link(path: &str, query_string: &str, after_cursor: &str) -> String {
    let mut pairs: Vec<(&str, &str)> = Vec::new();
    if !query_string.is_empty() {
        for pair in query_string.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (raw_key, raw_val) = match pair.split_once('=') {
                Some((k, v)) => (k, v),
                None => (pair, ""),
            };
            let decoded_key = percent_decode_str(raw_key).decode_utf8_lossy();
            if decoded_key == "page[after]" || decoded_key == "page[before]" {
                continue;
            }
            pairs.push((raw_key, raw_val));
        }
    }

    let encoded_cursor = utf8_percent_encode(after_cursor, CURSOR_ENCODE_SET).to_string();

    let mut qs = pairs
        .into_iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>();
    qs.push(format!("page[after]={}", encoded_cursor));

    format!("{}?{}", path, qs.join("&"))
}

/// Build the `PaginationLinks` for a list response.
///
/// The cursor-pagination profile requires `prev`/`next` to be present (but
/// nullable); this crate doesn't yet support backward pagination or
/// `first`/`last`, so `prev`, `first` and `last` are always `None`. `next` is
/// `Some` only when there's a further page and a cursor to build it from.
pub fn pagination_links(
    path: &str,
    query_string: &str,
    last_cursor: Option<&str>,
    has_more: bool,
) -> PaginationLinks {
    let next = (has_more && last_cursor.is_some())
        .then(|| next_page_link(path, query_string, last_cursor.unwrap()));
    PaginationLinks {
        prev: None,
        next,
        first: None,
        last: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Identifier, ResourceMeta, ResponseType};
    use crate::{Response, ID};

    #[derive(Debug, Clone, PartialEq, Deserialize, Default)]
    struct MyFilter {
        name: Option<StringMatch>,
        #[serde(default)]
        price: Range<u32>,
        #[serde(default)]
        include_deleted: bool,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum MySort {
        Name,
        CreatedAt,
    }

    fn happy_path_query() -> &'static str {
        "filter[name][contains]=shirt&filter[price][gte]=10&filter[price][lte]=20&sort=-created_at,name&page[size]=25&page[after]=abc123&include=author,comments.author"
    }

    #[test]
    fn full_happy_path() {
        let q: ListQuery<MyFilter, MySort> = parse_query(happy_path_query()).expect("should parse");
        assert_eq!(q.filter.name, Some(StringMatch::Contains("shirt".into())));
        assert_eq!(q.filter.price.gte, Some(10));
        assert_eq!(q.filter.price.lte, Some(20));
        assert!(!q.filter.include_deleted);
        assert_eq!(
            q.sort.0,
            vec![
                (MySort::CreatedAt, Direction::Desc),
                (MySort::Name, Direction::Asc)
            ]
        );
        assert_eq!(q.page.size, Some(25));
        assert_eq!(q.page.after, Some("abc123".to_owned()));
        assert!(q.include.contains("author"));
        assert!(q.include.contains("comments"));
        assert!(!q.include.contains("comments.author"));
    }

    #[test]
    fn form_encoded_happy_path_parses_identically() {
        let encoded = "filter%5Bname%5D%5Bcontains%5D=shirt&filter%5Bprice%5D%5Bgte%5D=10&filter%5Bprice%5D%5Blte%5D=20&sort=-created_at,name&page%5Bsize%5D=25&page%5Bafter%5D=abc123&include=author,comments.author";
        let plain: ListQuery<MyFilter, MySort> =
            parse_query(happy_path_query()).expect("should parse");
        let via_encoded: ListQuery<MyFilter, MySort> =
            parse_query(encoded).expect("should parse form-encoded");
        assert_eq!(plain.filter, via_encoded.filter);
        assert_eq!(plain.sort.0, via_encoded.sort.0);
        assert_eq!(plain.page, via_encoded.page);
        assert_eq!(plain.include, via_encoded.include);
    }

    #[test]
    fn empty_query_string_is_all_defaults() {
        let q: ListQuery<MyFilter, MySort> = parse_query("").expect("should parse");
        assert_eq!(q.filter, MyFilter::default());
        assert!(q.page.size.is_none());
        assert!(q.page.after.is_none());
        assert!(q.page.before.is_none());
        assert!(q.sort.is_empty());
        assert!(q.include.is_empty());
    }

    #[test]
    fn unknown_sort_key_is_error() {
        let err = parse_query::<ListQuery<MyFilter, MySort>>("sort=bogus_field")
            .expect_err("unknown sort key should fail");
        let msg = format!("{:?}", err);
        assert!(
            msg.contains("bogus_field") || err.detail.as_deref().unwrap_or("").contains("bogus_field"),
            "error should mention the offending segment: {:?}",
            err
        );
    }

    #[test]
    fn unsorted_rejects_sort_param_but_allows_absence() {
        let err = parse_query::<ListQuery<MyFilter, Unsorted>>("sort=name")
            .expect_err("sort should be rejected for Unsorted resources");
        assert!(err.detail.as_deref().unwrap_or("").contains("does not support sorting"));

        let ok = parse_query::<ListQuery<MyFilter, Unsorted>>("").expect("absent sort is fine");
        assert!(ok.sort.is_empty());
    }

    #[test]
    fn string_match_eq_vs_contains() {
        let eq: StringMatch = parse_query("eq=foo").expect("eq should parse");
        assert_eq!(eq, StringMatch::Eq("foo".into()));

        let contains: StringMatch = parse_query("contains=foo").expect("contains should parse");
        assert_eq!(contains, StringMatch::Contains("foo".into()));

        // As it appears nested under a filter field, e.g. `filter[name][contains]=foo`.
        let q: ListQuery<MyFilter, MySort> =
            parse_query("filter[name][eq]=foo").expect("nested eq should parse");
        assert_eq!(q.filter.name, Some(StringMatch::Eq("foo".into())));
    }

    #[test]
    fn range_is_empty() {
        let empty: Range<u32> = Range::default();
        assert!(empty.is_empty());

        let filled: Range<u32> = parse_query("gte=10").expect("should parse");
        assert!(!filled.is_empty());
        assert_eq!(filled.gte, Some(10));
    }

    #[test]
    fn cursor_page_from_probe_boundaries() {
        let empty: CursorPage<i32> = CursorPage::from_probe(vec![], 5);
        assert!(empty.items.is_empty());
        assert!(!empty.has_more);

        let exact = CursorPage::from_probe(vec![1, 2, 3, 4, 5], 5);
        assert_eq!(exact.items, vec![1, 2, 3, 4, 5]);
        assert!(!exact.has_more);

        let over = CursorPage::from_probe(vec![1, 2, 3, 4, 5, 6], 5);
        assert_eq!(over.items, vec![1, 2, 3, 4, 5]);
        assert!(over.has_more);
    }

    #[test]
    fn cursor_page_map_and_with_total() {
        let page = CursorPage::from_probe(vec![1, 2, 3], 5).with_total(Total::Exact(3));
        let mapped = page.map(|x| x * 2);
        assert_eq!(mapped.items, vec![2, 4, 6]);
        assert_eq!(mapped.total, Some(Total::Exact(3)));
    }

    #[test]
    fn next_page_link_preserves_other_params_and_order() {
        let link = next_page_link(
            "/widgets",
            "filter[name][eq]=foo&sort=-created_at&page[size]=10",
            "cur+sor/==",
        );
        assert_eq!(
            link,
            "/widgets?filter[name][eq]=foo&sort=-created_at&page[size]=10&page[after]=cur%2Bsor%2F%3D%3D"
        );
    }

    #[test]
    fn next_page_link_strips_existing_after_before_literal_and_encoded() {
        let link = next_page_link(
            "/widgets",
            "page[after]=old&filter[name][eq]=foo&page%5Bbefore%5D=older&sort=name",
            "newcursor",
        );
        assert_eq!(link, "/widgets?filter[name][eq]=foo&sort=name&page[after]=newcursor");
    }

    #[test]
    fn next_page_link_with_empty_query_string() {
        let link = next_page_link("/widgets", "", "abc");
        assert_eq!(link, "/widgets?page[after]=abc");
    }

    #[test]
    fn pagination_links_no_more_results() {
        let links = pagination_links("/widgets", "page[size]=10", Some("last"), false);
        assert!(links.next.is_none());
        assert!(links.prev.is_none());

        let links_no_cursor = pagination_links("/widgets", "page[size]=10", None, true);
        assert!(links_no_cursor.next.is_none());
    }

    #[test]
    fn pagination_links_has_more() {
        let links = pagination_links("/widgets", "page[size]=10", Some("last"), true);
        assert_eq!(
            links.next,
            Some("/widgets?page[size]=10&page[after]=last".to_owned())
        );
        assert!(links.prev.is_none());
    }

    #[derive(Debug, Clone, PartialEq, serde_derive::Serialize)]
    struct SimpleAttrs {
        name: String,
    }

    fn item(id: &str) -> crate::document::ResourceResponse<SimpleAttrs> {
        crate::document::ResourceResponse {
            id: Identifier {
                id: ID(id.to_owned()),
                typ: "widgets".to_owned(),
            },
            attributes: SimpleAttrs { name: id.to_owned() },
            relationships: None,
            meta: None,
        }
    }

    #[test]
    fn with_item_cursors_sets_meta() {
        let response: Response<SimpleAttrs, ()> = Response {
            primary: ResponseType::Ok(vec![item("1"), item("2")]),
            included: None,
            links: None,
            meta: None,
        };
        let response = response.with_item_cursors(|item| format!("cursor-{}", item.id.id.0));
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains(r#""cursor":"cursor-1""#));
        assert!(json.contains(r#""cursor":"cursor-2""#));
    }

    #[test]
    fn with_id_cursors_uses_resource_id() {
        let response: Response<SimpleAttrs, ()> = Response {
            primary: ResponseType::Ok(vec![item("abc")]),
            included: None,
            links: None,
            meta: None,
        };
        let response = response.with_id_cursors();
        match response.primary {
            ResponseType::Ok(items) => {
                assert_eq!(
                    items[0].meta.as_ref().map(|m: &ResourceMeta| m.page.cursor.clone()),
                    Some("abc".to_owned())
                );
            }
            _ => panic!("expected Ok"),
        }
    }
}
