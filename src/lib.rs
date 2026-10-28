use std::collections::BTreeMap;
use std::fmt;

use xee_xpath::context::StaticContextBuilder;
use xee_xpath::{DocumentHandle, Documents, Queries, Query};
use xot::xmlname::NameStrInfo;
use xot::{Node, ValueType, Xot};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PatchKind {
	Add,
	Replace,
	Remove
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Position {
	Append,
	Prepend,
	Before,
	After
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WhitespaceMode {
	Before,
	After,
	Both
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PatchContent {
	Text(String),
	Nodes(Vec<String>)
}

impl PatchContent {
	fn text(&self) -> Result<&str> {
		match self {
			Self::Text(text) => Ok(text),
			Self::Nodes(_) => Err(Error::Operation("operation content must contain text only".to_string()))
		}
	}

	fn fragments(&self) -> impl Iterator<Item = &str> {
		match self {
			Self::Text(text) => std::slice::from_ref(text).iter().map(String::as_str),
			Self::Nodes(nodes) => nodes.iter().map(String::as_str)
		}
	}
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PatchOperation {
	kind: PatchKind,
	selector: String,
	r#type: Option<String>,
	position: Option<Position>,
	whitespace: Option<WhitespaceMode>,
	content: PatchContent,
	context: Node
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
	InvalidPatch(String),
	InvalidSelector(String),
	Xml(String),
	Xpath(String),
	InvalidTarget(String),
	Operation(String)
}

impl fmt::Display for Error {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::InvalidPatch(msg) => write!(f, "invalid patch: {msg}"),
			Self::InvalidSelector(msg) => write!(f, "invalid selector: {msg}"),
			Self::Xml(msg) => write!(f, "xml error: {msg}"),
			Self::Xpath(msg) => write!(f, "xpath error: {msg}"),
			Self::InvalidTarget(msg) => write!(f, "invalid target: {msg}"),
			Self::Operation(msg) => write!(f, "operation error: {msg}")
		}
	}
}

impl std::error::Error for Error {}

impl From<xot::Error> for Error {
	fn from(value: xot::Error) -> Self {
		Self::Xml(value.to_string())
	}
}

fn attribute_value(xot: &Xot, element: Node, attribute_name: &str) -> Option<String> {
	xot.attributes(element).iter().find_map(|(name, value)| {
		let (local, _) = xot.name_ns_str(name);
		if local == attribute_name {
			Some(value.clone())
		} else {
			None
		}
	})
}

fn validate_operation_attributes(xot: &Xot, element: Node, kind: &PatchKind) -> Result<()> {
	for (name, _) in xot.attributes(element).iter() {
		let (local, namespace) = xot.name_ns_str(name);
		if namespace.is_empty()
			&& !matches!(
				(kind, local),
				(PatchKind::Add, "sel")
					| (PatchKind::Add, "type")
					| (PatchKind::Add, "pos")
					| (PatchKind::Replace, "sel")
					| (PatchKind::Remove, "sel")
					| (PatchKind::Remove, "ws")
			) {
			return Err(Error::InvalidPatch(format!(
				"unsupported attribute '{local}' on patch operation"
			)));
		}
	}
	Ok(())
}

fn has_attribute(xot: &Xot, element: Node, attribute_name: &str) -> bool {
	attribute_value(xot, element, attribute_name).is_some()
}

fn collect_in_scope_namespaces(xot: &Xot, node: Node) -> (Option<String>, BTreeMap<String, String>) {
	let mut namespaces: BTreeMap<String, String> = BTreeMap::new();
	let mut default_namespace = None;
	let mut current = Some(node);
	let mut entries = Vec::new();

	while let Some(n) = current {
		if xot.is_element(n) {
			let mut local = Vec::new();
			for (prefix_id, namespace_id) in xot.namespaces(n).iter() {
				let prefix = xot.prefix_str(prefix_id).to_string();
				let uri = xot.namespace_str(*namespace_id).to_string();
				local.push((prefix, uri));
			}
			entries.push(local);
		}
		current = xot.parent(n);
	}

	for local in entries.iter().rev() {
		for (prefix, uri) in local {
			if prefix.is_empty() {
				default_namespace = Some(uri.clone());
			} else {
				namespaces.insert(prefix.clone(), uri.clone());
			}
		}
	}

	(default_namespace, namespaces)
}

fn build_selector_context<'a>(
	default_namespace: &'a Option<String>,
	bindings: &'a BTreeMap<String, String>
) -> StaticContextBuilder<'a> {
	let mut builder = StaticContextBuilder::default();
	if let Some(uri) = default_namespace {
		builder.default_element_namespace(uri);
	}
	for (prefix, uri) in bindings {
		builder.add_namespace(prefix, uri);
	}
	builder
}

fn parse_patch_document(patch_xml: &str) -> Result<(Xot, Vec<PatchOperation>)> {
	let mut xot = Xot::new();
	let document = xot.parse(patch_xml).map_err(|e| Error::Xml(e.to_string()))?;
	let root = xot.document_element(document).map_err(|e| Error::Xml(e.to_string()))?;

	let candidates: Vec<Node> = if let Some(root_element) = xot.element(root) {
		let (local, _) = xot.name_ns_str(root_element.name());
		if matches!(local, "add" | "replace" | "remove") {
			vec![root]
		} else {
			xot.children(root).filter(|child| xot.is_element(*child)).collect()
		}
	} else {
		Vec::new()
	};

	let mut operations = Vec::new();
	for candidate in candidates {
		if let Some(operation) = parse_operation(&xot, candidate)? {
			operations.push(operation);
		}
	}

	Ok((xot, operations))
}

fn parse_operation(xot: &Xot, candidate: Node) -> Result<Option<PatchOperation>> {
	let element = xot
		.element(candidate)
		.ok_or_else(|| Error::InvalidPatch("patch element is not an element".to_string()))?;
	let (local_name, _) = xot.name_ns_str(element.name());
	let kind = match local_name {
		"add" => PatchKind::Add,
		"replace" => PatchKind::Replace,
		"remove" => PatchKind::Remove,
		_ => return Ok(None)
	};
	let selector = attribute_value(xot, candidate, "sel")
		.ok_or_else(|| Error::InvalidPatch("selector attribute 'sel' is required".to_string()))?;
	validate_operation_attributes(xot, candidate, &kind)?;

	let position = parse_position(attribute_value(xot, candidate, "pos").as_deref())?;
	let whitespace = parse_whitespace(attribute_value(xot, candidate, "ws").as_deref())?;
	let has_position = has_attribute(xot, candidate, "pos");
	let has_whitespace = has_attribute(xot, candidate, "ws");
	if kind != PatchKind::Add && has_position {
		return Err(Error::InvalidPatch("'pos' is only valid on add operations".to_string()));
	}
	if kind != PatchKind::Remove && has_whitespace {
		return Err(Error::InvalidPatch(
			"'ws' is only valid on remove operations".to_string()
		));
	}

	Ok(Some(PatchOperation {
		kind,
		selector,
		r#type: attribute_value(xot, candidate, "type"),
		position: (kind == PatchKind::Add).then_some(position),
		whitespace: (kind == PatchKind::Remove).then_some(whitespace).flatten(),
		content: collect_patch_content(xot, candidate)?,
		context: candidate
	}))
}

fn parse_position(value: Option<&str>) -> Result<Position> {
	Ok(match value {
		Some("before") => Position::Before,
		Some("after") => Position::After,
		Some("prepend") => Position::Prepend,
		Some("append") | None => Position::Append,
		Some(value) => return Err(Error::InvalidPatch(format!("unknown pos value '{value}'")))
	})
}

fn parse_whitespace(value: Option<&str>) -> Result<Option<WhitespaceMode>> {
	Ok(match value {
		Some("before") => Some(WhitespaceMode::Before),
		Some("after") => Some(WhitespaceMode::After),
		Some("both") => Some(WhitespaceMode::Both),
		None => None,
		Some(value) => return Err(Error::InvalidPatch(format!("unknown ws value '{value}'")))
	})
}

fn collect_patch_content(xot: &Xot, element: Node) -> Result<PatchContent> {
	let mut fragments = Vec::new();
	let mut contains_non_text = false;
	for child in xot.children(element) {
		match xot.value_type(child) {
			ValueType::Text => {
				let text = xot.text_str(child).unwrap_or_default().to_string();
				fragments.push(text);
			}
			ValueType::Element | ValueType::Comment | ValueType::ProcessingInstruction => {
				contains_non_text = true;
				fragments.push(xot.to_string(child).map_err(|e| Error::Xml(e.to_string()))?);
			}
			_ => {}
		}
	}

	if fragments.is_empty() {
		Ok(PatchContent::Text(String::new()))
	} else if !contains_non_text {
		Ok(PatchContent::Text(fragments.concat()))
	} else {
		Ok(PatchContent::Nodes(fragments))
	}
}

fn select_unique(
	documents: &mut Documents,
	document: DocumentHandle,
	selector: &str,
	patch_xot: &Xot,
	patch_context: Node
) -> Result<Node> {
	let matches = evaluate_selector(documents, document, selector, patch_xot, patch_context)?;
	if matches.len() == 1 {
		Ok(matches[0])
	} else {
		Err(Error::InvalidSelector(format!(
			"selector '{selector}' must resolve to one node"
		)))
	}
}

fn collect_content_nodes(documents: &mut Documents, content: &PatchContent) -> Result<Vec<Node>> {
	let mut nodes = Vec::new();
	for fragment in content.fragments() {
		let (_, fragment_nodes) = collect_fragment_nodes(documents.xot_mut(), fragment)?;
		nodes.extend(fragment_nodes);
	}
	Ok(nodes)
}

fn is_document_root(xot: &Xot, node: Node) -> bool {
	xot.is_element(node) && xot.parent(node).is_some_and(|parent| xot.is_document(parent))
}

fn is_whitespace_text(xot: &Xot, node: Node) -> bool {
	xot.is_text(node) && xot.text_str(node).unwrap_or("").trim().is_empty()
}

pub fn apply_patch(target_xml: &str, patch_xml: &str) -> Result<String> {
	let mut documents = Documents::new();
	let document_handle = documents
		.add_string_without_uri(target_xml)
		.map_err(|e| Error::Xml(e.to_string()))?;
	let document_root = documents
		.document_node(document_handle)
		.ok_or_else(|| Error::InvalidTarget("document root is missing".to_string()))?;

	let (patch_xot, patch_operations) = parse_patch_document(patch_xml)?;

	for operation in &patch_operations {
		apply_operation(&mut documents, document_handle, &patch_xot, operation)?;
	}

	let root = document_root;
	documents.xot().to_string(root).map_err(|e| Error::Xml(e.to_string()))
}

fn apply_operation(
	documents: &mut Documents,
	document: DocumentHandle,
	patch_xot: &Xot,
	operation: &PatchOperation
) -> Result<()> {
	match operation.kind {
		PatchKind::Add => apply_add_operation(documents, document, patch_xot, operation),
		PatchKind::Replace => apply_replace_operation(documents, document, patch_xot, operation),
		PatchKind::Remove => apply_remove_operation(documents, document, patch_xot, operation)
	}
}

fn evaluate_selector(
	documents: &mut Documents,
	document: DocumentHandle,
	selector: &str,
	xot: &Xot,
	element: Node
) -> Result<Vec<Node>> {
	if selector.contains("namespace-uri(") || selector.contains("local-name(") {
		return Err(Error::InvalidSelector(
			"namespace-uri() and local-name() are not allowed in RFC 5261 selectors".to_string()
		));
	}
	if let Some((parent_selector, prefix)) = selector.rsplit_once("/namespace::") {
		if parent_selector.is_empty() || prefix.is_empty() || prefix.contains(':') {
			return Err(Error::InvalidSelector(format!(
				"invalid namespace selector '{selector}'"
			)));
		}
		let parents = evaluate_selector(documents, document, parent_selector, xot, element)?;
		if parents.len() != 1 {
			return Err(Error::InvalidSelector(format!(
				"selector '{parent_selector}' must resolve to one node"
			)));
		}
		let parent = parents[0];
		if !documents.xot().is_element(parent) {
			return Err(Error::InvalidSelector(
				"namespace axis requires an element target".to_string()
			));
		}
		let prefix_id = documents.xot_mut().add_prefix(prefix);
		let namespace_id = documents.xot().get_namespace(parent, prefix_id).ok_or_else(|| {
			Error::InvalidSelector(format!("namespace '{prefix}' is not declared on the selected element"))
		})?;
		let namespace_node = documents.xot_mut().new_namespace_node(prefix_id, namespace_id);
		let namespace_node = documents
			.xot_mut()
			.append_namespace_node(parent, namespace_node)
			.map_err(Error::from)?;
		return Ok(vec![namespace_node]);
	}
	if let Some((parent_selector, test)) = selector.rsplit_once("/processing-instruction") {
		if !(test == "()" || (test.starts_with('(') && test.ends_with(')'))) {
			return Err(Error::InvalidSelector(format!(
				"invalid processing-instruction selector '{selector}'"
			)));
		}
		let parents = if parent_selector.is_empty() {
			let root = documents
				.document_node(document)
				.ok_or_else(|| Error::InvalidTarget("document root is missing".to_string()))?;
			vec![documents.xot().parent(root).unwrap_or(root)]
		} else {
			evaluate_selector(documents, document, parent_selector, xot, element)?
		};
		if parents.len() != 1 {
			return Err(Error::InvalidSelector(format!(
				"selector '{parent_selector}' must resolve to one node"
			)));
		}
		let target_name = test
			.strip_prefix("(\"")
			.or_else(|| test.strip_prefix("('"))
			.and_then(|value| value.strip_suffix("\")").or_else(|| value.strip_suffix("')")));
		let nodes: Vec<Node> = documents
			.xot()
			.children(parents[0])
			.filter(|node| {
				if !documents.xot().is_processing_instruction(*node) {
					return false;
				}
				target_name.is_none_or(|name| {
					documents
						.xot()
						.processing_instruction(*node)
						.is_some_and(|pi| documents.xot().local_name_str(pi.target()) == name)
				})
			})
			.collect();
		return Ok(nodes);
	}
	let (default_namespace, bindings) = collect_in_scope_namespaces(xot, element);
	let builder = build_selector_context(&default_namespace, &bindings);
	let queries = Queries::new(builder);
	let query = queries.sequence(selector).map_err(|e| Error::Xpath(e.to_string()))?;
	let sequence = query
		.execute(documents, document)
		.map_err(|e| Error::Xpath(e.to_string()))?;
	let mut nodes = Vec::new();
	for item in sequence.iter() {
		match item {
			xee_xpath::Item::Node(node) => nodes.push(node),
			_ => {
				return Err(Error::InvalidSelector(format!(
					"selector '{selector}' did not select nodes"
				)));
			}
		}
	}
	Ok(nodes)
}

fn split_qname(value: &str) -> Result<(&str, &str)> {
	let mut parts = value.split(':');
	let first = parts.next().unwrap_or_default();
	let second = parts.next();
	if first.is_empty() || second.is_some_and(str::is_empty) || parts.next().is_some() {
		return Err(Error::Operation(format!("invalid QName '{value}'")));
	}
	Ok(match second {
		Some(local) => (first, local),
		None => ("", first)
	})
}

fn namespace_for_prefix(xot: &Xot, context: Node, prefix: &str, default_allowed: bool) -> Result<String> {
	if prefix.is_empty() && !default_allowed {
		return Ok(String::new());
	}
	let (default_namespace, bindings) = collect_in_scope_namespaces(xot, context);
	if prefix.is_empty() {
		return default_namespace
			.ok_or_else(|| Error::InvalidPatch("unqualified name has no default namespace".to_string()));
	}
	bindings
		.get(prefix)
		.cloned()
		.ok_or_else(|| Error::InvalidPatch(format!("namespace prefix '{prefix}' is undeclared")))
}

fn choose_prefix(xot: &Xot, target_context: Node, source_prefix: &str, namespace: &str, attribute: bool) -> String {
	let (default_namespace, bindings) = collect_in_scope_namespaces(xot, target_context);
	if !attribute && source_prefix.is_empty() && default_namespace.as_deref() == Some(namespace) {
		return String::new();
	}
	if !source_prefix.is_empty() && bindings.get(source_prefix).is_some_and(|uri| uri == namespace) {
		return source_prefix.to_string();
	}
	let mut candidates: Vec<_> = bindings
		.iter()
		.filter(|(_, uri)| uri.as_str() == namespace)
		.map(|(prefix, _)| prefix.clone())
		.collect();
	candidates.sort();
	candidates
		.into_iter()
		.next()
		.unwrap_or_else(|| source_prefix.to_string())
}

fn ensure_namespace_for_node(xot: &mut Xot, node: Node, target_context: Node) -> Result<()> {
	if xot.is_element(node) {
		let name = xot
			.node_name_ref(node)
			.map_err(|e| Error::Xml(e.to_string()))?
			.ok_or_else(|| Error::InvalidTarget("element name is missing".to_string()))?;
		let namespace = name.namespace().to_string();
		if !namespace.is_empty() {
			let source_prefix = name.prefix().to_string();
			let prefix = choose_prefix(xot, target_context, &source_prefix, &namespace, false);
			let prefix_id = xot.add_prefix(&prefix);
			let namespace_id = xot.add_namespace(&namespace);
			xot.set_namespace(node, prefix_id, namespace_id);
		}

		let attributes: Vec<_> = xot
			.attributes(node)
			.iter()
			.map(|(name, value)| (name, value.clone()))
			.collect();
		for (name_id, value) in attributes {
			let (local_name, namespace, source_prefix) = {
				let name = xot.name_ref(name_id, node).map_err(|e| Error::Xml(e.to_string()))?;
				(
					name.local_name().to_string(),
					name.namespace().to_string(),
					name.prefix().to_string()
				)
			};
			if !namespace.is_empty() {
				let prefix = choose_prefix(xot, target_context, &source_prefix, &namespace, true);
				let prefix_id = xot.add_prefix(&prefix);
				let namespace_id = xot.add_namespace(&namespace);
				let new_name = xot.add_name_ns(&local_name, namespace_id);
				xot.remove_attribute(node, name_id);
				xot.set_attribute(node, new_name, value);
				xot.set_namespace(node, prefix_id, namespace_id);
			}
		}
	}

	let children: Vec<_> = xot.children(node).collect();
	for child in children {
		if xot.is_element(child) {
			ensure_namespace_for_node(xot, child, target_context)?;
		}
	}
	prune_unused_namespaces(xot, node, target_context)?;
	Ok(())
}

fn prune_unused_namespaces(xot: &mut Xot, node: Node, target_context: Node) -> Result<()> {
	let mut used = BTreeMap::new();
	collect_used_namespaces(xot, node, &mut used)?;
	let (target_default, target_bindings) = collect_in_scope_namespaces(xot, target_context);
	let declarations: Vec<_> = xot
		.namespaces(node)
		.iter()
		.map(|(prefix, namespace)| (prefix, *namespace))
		.collect();
	for (prefix, namespace) in declarations {
		let prefix_name = xot.prefix_str(prefix);
		let namespace_name = xot.namespace_str(namespace);
		let is_used = used.get(prefix_name).is_some_and(|uri| uri == namespace_name);
		let target_has_namespace = if prefix_name.is_empty() {
			target_default.as_deref() == Some(namespace_name)
		} else {
			target_bindings.values().any(|uri| uri == namespace_name)
		};
		if !is_used || target_has_namespace {
			xot.remove_namespace(node, prefix);
		}
	}
	Ok(())
}

fn collect_used_namespaces(xot: &Xot, node: Node, used: &mut BTreeMap<String, String>) -> Result<()> {
	if xot.is_element(node) {
		let name = xot
			.node_name_ref(node)
			.map_err(|e| Error::Xml(e.to_string()))?
			.ok_or_else(|| Error::InvalidTarget("element name is missing".to_string()))?;
		if !name.namespace().is_empty() {
			used.insert(name.prefix().to_string(), name.namespace().to_string());
		}
		for (name_id, _) in xot.attributes(node).iter() {
			let name = xot.name_ref(name_id, node).map_err(|e| Error::Xml(e.to_string()))?;
			if !name.namespace().is_empty() {
				used.insert(name.prefix().to_string(), name.namespace().to_string());
			}
		}
	}
	for child in xot.children(node).filter(|child| xot.is_element(*child)) {
		collect_used_namespaces(xot, child, used)?;
	}
	Ok(())
}

fn target_name(
	target_xot: &mut Xot,
	patch_xot: &Xot,
	patch_context: Node,
	target_context: Node,
	value: &str,
	attribute: bool
) -> Result<xot::NameId> {
	let (prefix, local) = split_qname(value)?;
	let namespace = namespace_for_prefix(patch_xot, patch_context, prefix, !attribute)?;
	let prefix = choose_prefix(target_xot, target_context, prefix, &namespace, attribute);
	let namespace_id = target_xot.add_namespace(&namespace);
	let name = target_xot.add_name_ns(local, namespace_id);
	if !prefix.is_empty() {
		let prefix_id = target_xot.add_prefix(&prefix);
		target_xot.set_namespace(target_context, prefix_id, namespace_id);
	}
	Ok(name)
}

fn apply_add_operation(
	documents: &mut Documents,
	document: DocumentHandle,
	patch_xot: &Xot,
	operation: &PatchOperation
) -> Result<()> {
	let target = select_unique(documents, document, &operation.selector, patch_xot, operation.context)?;

	if let Some(attr_name) = operation.r#type.as_deref() {
		if let Some(name) = attr_name.strip_prefix('@') {
			if name.is_empty() || name.starts_with('@') {
				return Err(Error::Operation(format!("invalid attribute name '{attr_name}'")));
			}
			let value = operation.content.text()?;
			if !documents.xot().is_element(target) {
				return Err(Error::Operation(
					"type '@name' can only be applied to an element target".to_string()
				));
			}
			let name_id = target_name(documents.xot_mut(), patch_xot, operation.context, target, name, true)?;
			documents.xot_mut().set_attribute(target, name_id, value.to_string());
			return Ok(());
		}
		if let Some(prefix) = attr_name.strip_prefix("namespace::") {
			if prefix.is_empty() || prefix.contains(':') {
				return Err(Error::Operation(format!("invalid namespace prefix '{prefix}'")));
			}
			let value = operation.content.text()?;
			if value.is_empty() {
				return Err(Error::Operation("namespace URI must not be empty".to_string()));
			}
			if !documents.xot().is_element(target) {
				return Err(Error::Operation(
					"type 'namespace::prefix' can only be applied to an element target".to_string()
				));
			}
			let prefix_id = documents.xot_mut().add_prefix(prefix);
			let ns_id = documents.xot_mut().add_namespace(value);
			documents.xot_mut().set_namespace(target, prefix_id, ns_id);
			return Ok(());
		}
		return Err(Error::Operation(format!("unsupported type '{attr_name}'")));
	}

	let fragments = operation.content.fragments();

	let position = operation.position.unwrap_or(Position::Append);
	let parent = if documents.xot().is_element(target) {
		target
	} else {
		documents
			.xot()
			.parent(target)
			.ok_or_else(|| Error::InvalidTarget("target has no parent".to_string()))?
	};

	let mut nodes_to_insert = Vec::new();
	for fragment in fragments {
		let (_wrapper, nodes) = collect_fragment_nodes(documents.xot_mut(), fragment)?;
		for node in nodes {
			ensure_namespace_for_node(documents.xot_mut(), node, parent)?;
			nodes_to_insert.push(node);
		}
	}
	if matches!(position, Position::Prepend) {
		nodes_to_insert.reverse();
	}
	let mut reference = target;
	for node in nodes_to_insert {
		if matches!(position, Position::Before | Position::After)
			&& documents.xot().is_element(node)
			&& (is_document_root(documents.xot(), target) || documents.xot().is_document(parent))
		{
			return Err(Error::Operation(
				"cannot add an element sibling to the document root".to_string()
			));
		}
		match position {
			Position::Append => documents.xot_mut().append(parent, node)?,
			Position::Prepend => {
				if let Some(first) = documents.xot().first_child(parent) {
					documents.xot_mut().insert_before(first, node)?;
				} else {
					documents.xot_mut().append(parent, node)?;
				}
			}
			Position::Before => documents.xot_mut().insert_before(target, node)?,
			Position::After => {
				documents.xot_mut().insert_after(reference, node)?;
				reference = documents.xot().next_sibling(reference).unwrap_or(reference);
			}
		}
	}

	Ok(())
}

fn apply_replace_operation(
	documents: &mut Documents,
	document: DocumentHandle,
	patch_xot: &Xot,
	operation: &PatchOperation
) -> Result<()> {
	let target = select_unique(documents, document, &operation.selector, patch_xot, operation.context)?;
	if is_document_root(documents.xot(), target) {
		return Err(Error::Operation("document root element cannot be replaced".to_string()));
	}

	if documents.xot().is_attribute_node(target) {
		let parent = documents
			.xot()
			.parent(target)
			.ok_or_else(|| Error::InvalidTarget("attribute has no parent".to_string()))?;
		let name = match documents.xot().value(target) {
			xot::Value::Attribute(attr) => attr.name(),
			_ => {
				return Err(Error::InvalidTarget("attribute node is missing metadata".to_string()));
			}
		};
		let value = operation.content.text()?;
		documents.xot_mut().set_attribute(parent, name, value.to_string());
		return Ok(());
	}

	if documents.xot().is_namespace_node(target) {
		let prefix = match documents.xot().value(target) {
			xot::Value::Namespace(ns) => ns.prefix(),
			_ => {
				return Err(Error::InvalidTarget("namespace node is missing metadata".to_string()));
			}
		};
		let value = operation.content.text()?;
		let uri = documents.xot_mut().add_namespace(value);
		let parent = documents
			.xot()
			.parent(target)
			.ok_or_else(|| Error::InvalidTarget("namespace node has no parent".to_string()))?;
		if documents.xot().get_namespace(parent, prefix).is_none() {
			return Err(Error::Operation(
				"namespace declaration must be declared on its selected element".to_string()
			));
		}
		documents.xot_mut().set_namespace(parent, prefix, uri);
		return Ok(());
	}

	let replacement = collect_content_nodes(documents, &operation.content)?;

	if replacement.is_empty() {
		if documents.xot().is_text(target) {
			documents.xot_mut().remove(target)?;
			return Ok(());
		}
		return Err(Error::Operation(
			"replace requires content of the same node type".to_string()
		));
	}

	if replacement.len() > 1 {
		return Err(Error::Operation(
			"replace requires a single replacement node".to_string()
		));
	}

	let replacement_node = replacement
		.into_iter()
		.next()
		.ok_or_else(|| Error::Operation("replace requires a single replacement node".to_string()))?;
	if documents.xot().is_element(target) {
		if !documents.xot().is_element(replacement_node) {
			return Err(Error::Operation(
				"replace requires an element replacement for an element target".to_string()
			));
		}
		if is_document_root(documents.xot(), replacement_node) {
			return Err(Error::Operation("document root element cannot be replaced".to_string()));
		}
		let parent = documents
			.xot()
			.parent(target)
			.ok_or_else(|| Error::InvalidTarget("element has no parent".to_string()))?;
		ensure_namespace_for_node(documents.xot_mut(), replacement_node, parent)?;
		documents.xot_mut().replace(target, replacement_node)?;
		return Ok(());
	}
	if documents.xot().is_comment(target) {
		if !documents.xot().is_comment(replacement_node) {
			return Err(Error::Operation(
				"replace requires a comment replacement for a comment target".to_string()
			));
		}
		documents.xot_mut().replace(target, replacement_node)?;
		return Ok(());
	}
	if documents.xot().is_processing_instruction(target) {
		if !documents.xot().is_processing_instruction(replacement_node) {
			return Err(Error::Operation(
				"replace requires a processing-instruction replacement for a processing-instruction target".to_string()
			));
		}
		documents.xot_mut().replace(target, replacement_node)?;
		return Ok(());
	}
	if documents.xot().is_text(target) {
		if !documents.xot().is_text(replacement_node) {
			return Err(Error::Operation(
				"replace requires text content for a text target".to_string()
			));
		}
		let text = documents
			.xot_mut()
			.text_mut(target)
			.ok_or_else(|| Error::InvalidTarget("text node is missing content".to_string()))?;
		text.set(operation.content.text()?);
		return Ok(());
	}
	documents.xot_mut().replace(target, replacement_node)?;
	Ok(())
}

fn apply_remove_operation(
	documents: &mut Documents,
	document: DocumentHandle,
	patch_xot: &Xot,
	operation: &PatchOperation
) -> Result<()> {
	let target = select_unique(documents, document, &operation.selector, patch_xot, operation.context)?;
	if is_document_root(documents.xot(), target) {
		return Err(Error::Operation("document root element cannot be removed".to_string()));
	}
	if documents.xot().is_attribute_node(target) {
		if operation.whitespace.is_some() {
			return Err(Error::Operation(
				"ws is not allowed when removing attributes".to_string()
			));
		}
		let parent = documents
			.xot()
			.parent(target)
			.ok_or_else(|| Error::InvalidTarget("attribute has no parent".to_string()))?;
		let name = match documents.xot().value(target) {
			xot::Value::Attribute(attr) => attr.name(),
			_ => {
				return Err(Error::InvalidTarget("attribute node is missing metadata".to_string()));
			}
		};
		documents.xot_mut().remove_attribute(parent, name);
		return Ok(());
	}

	if documents.xot().is_namespace_node(target) {
		if operation.whitespace.is_some() {
			return Err(Error::Operation(
				"ws is not allowed when removing namespaces".to_string()
			));
		}
		let parent = documents
			.xot()
			.parent(target)
			.ok_or_else(|| Error::InvalidTarget("namespace node has no parent".to_string()))?;
		let prefix = match documents.xot().value(target) {
			xot::Value::Namespace(ns) => ns.prefix(),
			_ => {
				return Err(Error::InvalidTarget("namespace node has no metadata".to_string()));
			}
		};
		if documents.xot().get_namespace(parent, prefix).is_none() {
			return Err(Error::Operation(
				"namespace declaration must be declared on its selected element".to_string()
			));
		}
		documents.xot_mut().remove_namespace(parent, prefix);
		return Ok(());
	}

	if documents.xot().is_text(target) {
		if operation.whitespace.is_some() {
			return Err(Error::Operation("ws is not allowed when removing text".to_string()));
		}
		documents.xot_mut().remove(target)?;
		return Ok(());
	}

	if let Some(ws_mode) = &operation.whitespace {
		let prev = documents.xot().previous_sibling(target);
		let next = documents.xot().next_sibling(target);
		let before = prev.filter(|node| is_whitespace_text(documents.xot(), *node));
		let after = next.filter(|node| is_whitespace_text(documents.xot(), *node));
		let valid = match ws_mode {
			WhitespaceMode::Before => before.is_some(),
			WhitespaceMode::After => after.is_some(),
			WhitespaceMode::Both => before.is_some() && after.is_some()
		};
		if !valid {
			return Err(Error::Operation(
				"ws directive did not match a whitespace text sibling".to_string()
			));
		}
		if matches!(ws_mode, WhitespaceMode::Before | WhitespaceMode::Both)
			&& let Some(before) = before
		{
			documents.xot_mut().remove(before)?;
		}
		if matches!(ws_mode, WhitespaceMode::After | WhitespaceMode::Both)
			&& let Some(after) = after
		{
			documents.xot_mut().remove(after)?;
		}
	}

	if documents.xot().is_element(target) {
		let prev = documents.xot().previous_sibling(target);
		let next = documents.xot().next_sibling(target);
		if prev.is_some_and(|node| is_whitespace_text(documents.xot(), node))
			&& next.is_some_and(|node| is_whitespace_text(documents.xot(), node))
		{
			let (Some(prev_text), Some(next_text)) = (prev, next) else {
				return Err(Error::InvalidTarget(
					"element siblings changed while removing whitespace".to_string()
				));
			};
			let mut merged = documents.xot().text_str(prev_text).unwrap_or("").to_string();
			merged.push_str(documents.xot().text_str(next_text).unwrap_or(""));
			let Some(text) = documents.xot_mut().text_mut(prev_text) else {
				return Err(Error::InvalidTarget(
					"whitespace sibling is missing text metadata".to_string()
				));
			};
			text.set(&merged);
			documents.xot_mut().remove(next_text)?;
		}
	}

	documents.xot_mut().remove(target)?;
	Ok(())
}

fn collect_fragment_nodes(xot: &mut Xot, fragment: &str) -> Result<(Node, Vec<Node>)> {
	if fragment.is_empty() {
		let document = xot.parse("<root/>").map_err(|e| Error::Xml(e.to_string()))?;
		let wrapper = xot.document_element(document).map_err(|e| Error::Xml(e.to_string()))?;
		return Ok((wrapper, Vec::new()));
	}

	let wrapped = format!("<root>{fragment}</root>");
	let document = xot.parse(&wrapped).map_err(|e| Error::Xml(e.to_string()))?;
	let wrapper = xot.document_element(document).map_err(|e| Error::Xml(e.to_string()))?;
	let mut nodes = Vec::new();
	for child in xot.children(wrapper) {
		nodes.push(child);
	}
	Ok((wrapper, nodes))
}

#[cfg(test)]
mod tests;
