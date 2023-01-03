use std::collections::HashMap;
use std::rc::Rc;

use anyhow::Context;
use anyhow::Result;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor, Range};

#[derive(Debug, Builder)]
struct PreTag {
    id: usize,
    ancestor_ids: Vec<usize>,
    name: String,
    kind: Rc<String>,
    range: Range,
}

#[derive(Debug)]
pub struct Tag {
    pub name: String,
    pub parent: Option<Rc<Tag>>,
    pub kind: Rc<String>,
}

impl Tag {
    pub fn new(parent: Rc<Tag>, name: String, kind: Rc<String>) -> Self {
        Self { name, parent: Some(parent), kind }
    }

    pub fn new_root(name: String, kind: Rc<String>) -> Self {
        Self { name, parent: None, kind }
    }
}

#[derive(Debug)]
pub struct LocalTag {
    pub tag: Rc<Tag>,
    pub range: Range,
}

impl LocalTag {
    pub fn new(tag: Rc<Tag>, range: Range) -> Self {
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

fn to_local_tags(mut pre_tags: Vec<PreTag>) -> Vec<LocalTag> {
    pre_tags.sort_by_key(|t| t.ancestor_ids.len());

    let mut tags: HashMap<usize, Rc<Tag>> = HashMap::new();
    let mut local_tags = Vec::new();

    for pre_tag in pre_tags {
        let parent_id = pre_tag.ancestor_ids.iter().find(|&id| tags.contains_key(id));

        let tag = match parent_id {
            Some(parent_id) => match tags.get(parent_id) {
                Some(parent_tag) => Tag::new(parent_tag.clone(), pre_tag.name, pre_tag.kind),
                None => unreachable!(),
            },
            None => Tag::new_root(pre_tag.name, pre_tag.kind),
        };

        let tag = Rc::new(tag);
        local_tags.push(LocalTag::new(tag.clone(), pre_tag.range));
        tags.insert(pre_tag.id, tag);
    }

    local_tags
}

pub struct TagGenerator {
    parser: Parser,
    query: Query,
    name_ix: u32,
    tag_kinds: Vec<Option<Rc<String>>>,
}

impl TagGenerator {
    pub fn new<Q: AsRef<str>>(language: Language, query: Q) -> Result<Self> {
        let mut parser = Parser::new();
        parser.set_language(language)?;
        let query = Query::new(language, query.as_ref()).context("failed to parse query")?;

        let name_ix =
            query.capture_index_for_name("name").context("failed to find `name` capture")?;
        let tag_kinds: Vec<Option<Rc<String>>> = query
            .capture_names()
            .iter()
            .map(|n| n.strip_prefix("tag.").map(|n| Rc::new(n.to_string())))
            .collect();

        Ok(Self { parser, query, name_ix, tag_kinds })
    }

    pub fn generate_tags<S: AsRef<str>>(&mut self, source_code: S) -> Result<Vec<LocalTag>> {
        self.parser.reset();
        let tree =
            self.parser.parse(source_code.as_ref(), None).context("failed to parse source code")?;
        let mut cursor = QueryCursor::new();
        let source_bytes = source_code.as_ref().as_bytes();

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

        Ok(to_local_tags(pre_tags))
    }
}