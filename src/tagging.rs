use std::borrow::BorrowMut;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor, Range};

#[derive(Debug, Builder)]
struct PreTag {
    id: usize,
    ancestor_ids: Vec<usize>,
    name: String,
    kind: Arc<String>,
    range: Range,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Tag {
    pub name: String,
    pub parent: Option<Arc<Tag>>,
    pub kind: Arc<String>,
}

impl Tag {
    pub fn new(parent: Arc<Tag>, name: String, kind: Arc<String>) -> Self {
        Self { name, parent: Some(parent), kind }
    }

    pub fn new_root(name: String, kind: Arc<String>) -> Self {
        Self { name, parent: None, kind }
    }

    pub fn to_vec(&self) -> Vec<(String, Arc<String>)> {
        let mut ancestors = vec![(self.name.clone(), self.kind.clone())];
        let mut current = &self.parent;

        while let Some(tag) = current {
            ancestors.push((tag.name.clone(), tag.kind.clone()));
            current = &tag.parent;
        }

        ancestors.reverse();
        ancestors
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LocalTag {
    pub tag: Arc<Tag>,
    pub range: Range,
}

impl LocalTag {
    pub fn new(tag: Arc<Tag>, range: Range) -> Self {
        Self { tag, range }
    }
}

fn get_ancestor_ids(node: &Node) -> Vec<usize> {
    let mut ids = Vec::new();
    let mut curr: Option<Node> = node.parent();

    while let Some(curr_node) = curr {
        ids.push(curr_node.id());
        curr = curr_node.parent();
    }

    ids
}

fn to_local_tags(file_tag: LocalTag, mut pre_tags: Vec<PreTag>) -> Vec<LocalTag> {
    pre_tags.sort_by_key(|t| t.ancestor_ids.len());

    let mut tags: HashMap<usize, Arc<Tag>> = HashMap::new();
    let mut local_tags = Vec::new();

    for pre_tag in pre_tags {
        let parent_id = pre_tag.ancestor_ids.iter().find(|&id| tags.contains_key(id));

        let tag = match parent_id {
            Some(parent_id) => match tags.get(parent_id) {
                Some(parent_tag) => Tag::new(parent_tag.clone(), pre_tag.name, pre_tag.kind),
                None => unreachable!(),
            },
            None => Tag::new(file_tag.tag.clone(), pre_tag.name, pre_tag.kind),
        };

        let tag = Arc::new(tag);
        local_tags.push(LocalTag::new(tag.clone(), pre_tag.range));
        tags.insert(pre_tag.id, tag);
    }

    local_tags.push(file_tag);
    local_tags
}

// fn push_root(local_tags: &mut Vec<LocalTag>, root: LocalTag) {
//     for tag in local_tags.iter().map(|t| t.tag.borrow_mut()) {
//         if tag.parent.is_none() {
//             tag.parent = Some(root.tag.clone());
//         }
//     }

//     local_tags.push(root);
// }

pub struct TagGenerator {
    parser: Parser,
    query: Query,
    name_ix: u32,
    tag_kinds: Vec<Option<Arc<String>>>,
}

impl TagGenerator {
    pub fn new<Q: AsRef<str>>(language: Language, query: Q) -> Result<Self> {
        let mut parser = Parser::new();
        parser.set_language(language)?;
        let query = Query::new(language, query.as_ref()).context("failed to parse query")?;

        let name_ix =
            query.capture_index_for_name("name").context("failed to find `name` capture")?;

        let tag_kinds = query
            .capture_names()
            .iter()
            .map(|n| n.strip_prefix("tag.").map(|n| Arc::new(n.to_string())))
            .collect::<Vec<_>>();

        Ok(Self { parser, query, name_ix, tag_kinds })
    }

    pub fn generate_tags(&mut self, filename: &String, source_code: impl AsRef<[u8]>) -> Result<Vec<LocalTag>> {
        self.parser.reset();
        let tree =
            self.parser.parse(source_code.as_ref(), None).context("failed to parse source code")?;
        let mut cursor = QueryCursor::new();
        let source_bytes = source_code.as_ref();

        let mut pre_tags = Vec::new();

        for r#match in cursor.matches(&self.query, tree.root_node(), source_bytes) {
            let mut builder = PreTagBuilder::default();

            for capture in r#match.captures {
                if capture.index == self.name_ix {
                    builder.name(capture.node.utf8_text(source_bytes).unwrap().to_string());
                    continue;
                }

                if let Some(tag_kind) = &self.tag_kinds[capture.index as usize] {
                    builder.id(capture.node.id());
                    builder.ancestor_ids(get_ancestor_ids(&capture.node));
                    builder.kind(tag_kind.clone());
                    builder.range(capture.node.range());
                    continue;
                }

                log::warn!("found unused capture");
            }

            pre_tags.push(builder.build()?);
        }

        // Create a "psuedo-tag" for the file to be the root tag
        let file_tag = Tag::new_root(filename.clone(), Arc::new("file".to_string()));
        let file_tag = LocalTag::new(Arc::new(file_tag), tree.root_node().range());

        Ok(to_local_tags(file_tag, pre_tags))
    }
}
