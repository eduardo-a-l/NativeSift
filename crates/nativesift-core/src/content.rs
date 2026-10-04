use parking_lot::Mutex;
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, Query, RegexQuery, TermQuery};
use tantivy::schema::{
    Field, IndexRecordOption, Schema, TantivyDocument, Value, STORED, STRING, TEXT,
};
use tantivy::{doc, Index, IndexReader, IndexWriter, ReloadPolicy, Term};

const WRITER_THREADS: usize = 2;
const WRITER_HEAP_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct ContentHit {
    pub path: String,
    pub score: f32,
}

pub struct ContentIndex {
    reader: IndexReader,
    writer: Mutex<IndexWriter>,
    path_field: Field,
    content_field: Field,
}

impl ContentIndex {
    pub fn new() -> tantivy::Result<Self> {
        let mut builder = Schema::builder();
        let path_field = builder.add_text_field("path", STRING | STORED);
        let content_field = builder.add_text_field("content", TEXT);
        let index = Index::create_in_ram(builder.build());
        let writer = index.writer_with_num_threads(WRITER_THREADS, WRITER_HEAP_BYTES)?;
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()?;
        Ok(Self {
            reader,
            writer: Mutex::new(writer),
            path_field,
            content_field,
        })
    }

    pub fn add(&self, path: &str, text: &str) -> tantivy::Result<()> {
        let writer = self.writer.lock();
        writer.add_document(doc!(self.path_field => path, self.content_field => text))?;
        Ok(())
    }

    pub fn replace(&self, path: &str, text: &str) -> tantivy::Result<()> {
        let writer = self.writer.lock();
        writer.delete_term(Term::from_field_text(self.path_field, path));
        writer.add_document(doc!(self.path_field => path, self.content_field => text))?;
        Ok(())
    }

    pub fn remove(&self, path: &str) {
        let writer = self.writer.lock();
        writer.delete_term(Term::from_field_text(self.path_field, path));
    }

    pub fn commit(&self) -> tantivy::Result<()> {
        self.writer.lock().commit()?;
        self.reader.reload()
    }

    pub fn search(&self, query: &str, limit: usize) -> tantivy::Result<Vec<ContentHit>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let Some(query) = self.build_query(query) else {
            return Ok(Vec::new());
        };
        let searcher = self.reader.searcher();
        let top = searcher.search(&query, &TopDocs::with_limit(limit))?;
        let mut hits = Vec::with_capacity(top.len());
        for (score, address) in top {
            let document: TantivyDocument = searcher.doc(address)?;
            if let Some(path) = document
                .get_first(self.path_field)
                .and_then(|value| value.as_str())
            {
                hits.push(ContentHit {
                    path: path.to_owned(),
                    score,
                });
            }
        }
        Ok(hits)
    }

    fn build_query(&self, raw: &str) -> Option<BooleanQuery> {
        let tokens: Vec<String> = raw
            .split(|character: char| !character.is_alphanumeric())
            .filter(|token| !token.is_empty())
            .map(str::to_lowercase)
            .collect();
        if tokens.is_empty() {
            return None;
        }
        let still_typing = raw.chars().last().is_some_and(char::is_alphanumeric);
        let last = tokens.len() - 1;

        let clauses = tokens
            .iter()
            .enumerate()
            .map(|(position, token)| {
                let exact = || -> Box<dyn Query> {
                    Box::new(TermQuery::new(
                        Term::from_field_text(self.content_field, token),
                        IndexRecordOption::Basic,
                    ))
                };
                let query: Box<dyn Query> = if position == last && still_typing {
                    RegexQuery::from_pattern(&format!("{token}.*"), self.content_field)
                        .map(|prefix| Box::new(prefix) as Box<dyn Query>)
                        .unwrap_or_else(|_| exact())
                } else {
                    exact()
                };
                (Occur::Must, query)
            })
            .collect();
        Some(BooleanQuery::new(clauses))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index_with(docs: &[(&str, &str)]) -> ContentIndex {
        let index = ContentIndex::new().unwrap();
        for (path, text) in docs {
            index.add(path, text).unwrap();
        }
        index.commit().unwrap();
        index
    }

    fn paths(hits: Vec<ContentHit>) -> Vec<String> {
        let mut paths: Vec<String> = hits.into_iter().map(|hit| hit.path).collect();
        paths.sort();
        paths
    }

    #[test]
    fn finds_documents_by_term() {
        let index = index_with(&[("/a.md", "alpha beta gamma"), ("/b.md", "delta epsilon")]);
        assert_eq!(paths(index.search("beta", 10).unwrap()), vec!["/a.md"]);
    }

    #[test]
    fn last_term_matches_as_prefix_while_typing() {
        let index = index_with(&[("/a.md", "fuzzy matching engine")]);
        assert_eq!(
            paths(index.search("fuzzy match", 10).unwrap()),
            vec!["/a.md"]
        );
        assert!(index.search("fuzzy match ", 10).unwrap().is_empty());
    }

    #[test]
    fn all_terms_are_required() {
        let index = index_with(&[("/a.md", "alpha beta"), ("/b.md", "alpha gamma")]);
        assert_eq!(
            paths(index.search("alpha gamma", 10).unwrap()),
            vec!["/b.md"]
        );
    }

    #[test]
    fn replace_and_remove_update_results() {
        let index = index_with(&[("/a.md", "old content")]);
        index.replace("/a.md", "fresh words").unwrap();
        index.commit().unwrap();
        assert!(index.search("old", 10).unwrap().is_empty());
        assert_eq!(paths(index.search("fresh", 10).unwrap()), vec!["/a.md"]);

        index.remove("/a.md");
        index.commit().unwrap();
        assert!(index.search("fresh", 10).unwrap().is_empty());
    }

    #[test]
    fn punctuation_only_and_empty_queries_return_nothing() {
        let index = index_with(&[("/a.md", "text")]);
        assert!(index.search("", 10).unwrap().is_empty());
        assert!(index.search("!!! ???", 10).unwrap().is_empty());
        assert!(index.search("text", 0).unwrap().is_empty());
    }

    #[test]
    fn matches_identifiers_split_on_punctuation() {
        let index = index_with(&[("/a.rs", "fn build_search_index() {}")]);
        assert_eq!(
            paths(index.search("search_index", 10).unwrap()),
            vec!["/a.rs"]
        );
    }
}
