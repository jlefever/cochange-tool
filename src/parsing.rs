use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use tree_sitter::Language;
use tree_sitter::Node;
use tree_sitter::Parser;
use tree_sitter::Query;
use tree_sitter::QueryCursor;
use tree_sitter::Range;

use crate::ir::Entity;
use crate::ir::Interval;
use crate::ir::LocEntity;

#[derive(Debug, Builder)]
struct Tag {
    id: usize,
    ancestor_ids: Vec<usize>,
    name: String,
    kind: Arc<String>,
    range: Range,
}

pub struct FileParser {
    parser: Parser,
    query: Query,
    name_ix: u32,
    tag_kinds: Vec<Option<Arc<String>>>,
}

impl FileParser {
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

    pub fn parse(&mut self, source: &[u8], filename: &String) -> Result<Vec<LocEntity>> {
        self.parser.reset();
        let tree = self.parser.parse(source, None).context("failed to parse source code")?;
        let mut cursor = QueryCursor::new();

        let mut pre_tags = Vec::new();

        for r#match in cursor.matches(&self.query, tree.root_node(), source) {
            let mut builder = TagBuilder::default();

            for capture in r#match.captures {
                if capture.index == self.name_ix {
                    builder.name(capture.node.utf8_text(source).unwrap().to_string());
                    continue;
                }

                if let Some(tag_kind) = &self.tag_kinds[capture.index as usize] {
                    builder.id(capture.node.id());
                    builder.ancestor_ids(get_ancestor_ids(&capture.node));
                    builder.kind(tag_kind.clone());
                    builder.range(capture.node.range());
                    continue;
                }

                log::warn!("Found unused capture: {:?}", capture);
            }

            pre_tags.push(builder.build()?);
        }

        // Create a "psuedo-entity" for the file to be the root entity
        let file_tag = Entity::new_root(filename.clone(), Arc::new("file".to_string()));
        let file_tag = LocEntity::new(Arc::new(file_tag), to_interval(&tree.root_node().range()));

        Ok(to_loc_entities(file_tag, pre_tags))
    }
}

fn to_interval(range: &Range) -> Interval {
    Interval(range.start_point.row + 1, range.end_point.row + 1)
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

fn to_loc_entities(file_tag: LocEntity, mut tags: Vec<Tag>) -> Vec<LocEntity> {
    tags.sort_by_key(|t| t.ancestor_ids.len());

    let mut entities: HashMap<usize, Arc<Entity>> = HashMap::new();
    let mut loc_entities = Vec::new();

    for pre_tag in tags {
        let parent_id = pre_tag.ancestor_ids.iter().find(|&id| entities.contains_key(id));

        let tag = match parent_id {
            Some(parent_id) => match entities.get(parent_id) {
                Some(parent_tag) => Entity::new(parent_tag.clone(), pre_tag.name, pre_tag.kind),
                None => unreachable!(),
            },
            None => Entity::new(file_tag.entity.clone(), pre_tag.name, pre_tag.kind),
        };

        let tag = Arc::new(tag);
        loc_entities.push(LocEntity::new(tag.clone(), to_interval(&pre_tag.range)));
        entities.insert(pre_tag.id, tag);
    }

    loc_entities.push(file_tag);
    loc_entities
}
