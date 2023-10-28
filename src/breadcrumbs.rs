#[derive(Debug, Clone)]
pub struct Breadcrumbs<'a>(&'a str);

/// Helper for displaying breadcrumb navigation over a slash separated path in a template.
#[derive(Debug, Clone)]
pub struct BreadcrumbsIterator<'a> {
    whole_path: &'a str,
    index: Option<usize>,
}

impl<'a> BreadcrumbsIterator<'a> {
    pub fn new(path: &'a str) -> Self {
        if path.is_empty() {
            BreadcrumbsIterator {
                whole_path: path,
                index: None,
            }
        } else {
            BreadcrumbsIterator {
                whole_path: path,
                index: Some(0),
            }
        }
    }
}

impl<'a> Iterator for BreadcrumbsIterator<'a> {
    type Item = Breadcrumb<'a>;

    fn next(&mut self) -> Option<Breadcrumb<'a>> {
        let index = self.index?;

        if let Some(next_slash) = self.whole_path[index..]
            .find('/')
            .map(|name_len| index + name_len)
        {
            self.index = Some(next_slash + 1); // Assume that slash is single byte
            Some(Breadcrumb {
                name: &self.whole_path[index..next_slash],
                link_url: &self.whole_path[..next_slash],
            })
        } else {
            self.index = None;
            Some(Breadcrumb {
                name: &self.whole_path[index..],
                link_url: self.whole_path,
            })
        }
    }
}

impl<'a> IntoIterator for &BreadcrumbsIterator<'a> {
    type Item = Breadcrumb<'a>;
    type IntoIter = BreadcrumbsIterator<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.clone()
    }
}

#[derive(Debug, Clone)]
pub struct Breadcrumb<'a> {
    pub name: &'a str,
    pub link_url: &'a str,
}

#[cfg(test)]
mod test {
    use super::*;
    use assert2::assert;
    use proptest::prop_assume;
    use test_strategy::proptest;

    #[test]
    fn empty_string() {
        assert!(BreadcrumbsIterator::new("").count() == 0);
    }

    #[proptest]
    fn behaves_almost_like_split(path: String) {
        prop_assume!(!path.is_empty(), "Split returns one item on empty input");
        let expected: Vec<_> = path.split('/').collect();
        let actual: Vec<_> = BreadcrumbsIterator::new(&path)
            .map(|crumb| crumb.name)
            .collect();

        assert!(actual == expected);
    }

    #[proptest]
    fn link_url_ends_with_name(path: String) {
        for crumb in BreadcrumbsIterator::new(&path) {
            assert!(crumb.link_url.ends_with(crumb.name));
        }
    }

    #[proptest]
    fn link_url_is_path_prefix(path: String) {
        for crumb in BreadcrumbsIterator::new(&path) {
            assert!(path.starts_with(crumb.link_url));
        }
    }
}
