use super::*;

#[test]
fn replaces_text_content() {
	let result = apply_patch(
		"<root><item>old</item></root>",
		r#"<replace sel="/root/item/text()">new</replace>"#
	)
	.unwrap();

	assert_eq!(result, "<root><item>new</item></root>");
}

#[test]
fn rejects_markup_when_adding_an_attribute() {
	let result = apply_patch("<root/>", r#"<add sel="/root" type="@value"><child/></add>"#);

	assert!(matches!(result, Err(Error::Operation(message)) if message == "operation content must contain text only"));
}

#[test]
fn removes_whitespace_before_a_node() {
	let result = apply_patch(
		"<root><first/>\n  <second/></root>",
		r#"<remove sel="/root/second" ws="before"/>"#
	)
	.unwrap();

	assert_eq!(result, "<root><first/></root>");
}

#[test]
fn applies_rfc_add_replace_and_remove_examples() {
	let result = apply_patch(
		"<doc><note>This is a sample document</note></doc>",
		r#"<diff>
            <add sel="doc"><foo id="ert4773">This is a new child</foo></add>
            <add sel="doc/foo[@id='ert4773']" type="@user">Bob</add>
            <replace sel="doc/foo[@id='ert4773']"><bar a="2"/></replace>
            <remove sel="doc/bar"/>
        </diff>"#
	)
	.unwrap();

	assert_eq!(result, "<doc><note>This is a sample document</note></doc>");
}

#[test]
fn namespace_prefixes_are_mangled_to_the_target_context() {
	let result = apply_patch(
		r#"<r:root xmlns:r="urn:root"/>"#,
		r#"<p:diff xmlns:p="urn:root" xmlns:a="urn:item">
            <add sel="p:root"><a:item a:kind="x"/></add>
        </p:diff>"#
	)
	.unwrap();

	assert!(result.contains("<a:item"), "{result}");
	assert!(result.contains("a:kind=\"x\""));
	assert!(result.contains("xmlns:a=\"urn:item\""));
}

#[test]
fn reuses_an_in_scope_target_prefix_for_added_namespaced_content() {
	let result = apply_patch(
		r#"<root xmlns:b="urn:item"/>"#,
		r#"<diff xmlns:a="urn:item"><add sel="/root"><a:item/></add></diff>"#
	)
	.unwrap();

	assert!(result.contains("<b:item"), "{result}");
	assert!(!result.contains("<a:item"));
}

#[test]
fn keeps_patch_and_target_namespace_tables_independent_for_selectors_and_attributes() {
	let result = apply_patch(
		r#"<t:target xmlns:t="urn:target" xmlns:unused="urn:unused"><t:item/></t:target>"#,
		r#"<p:diff xmlns:p="urn:target" xmlns:a="urn:attribute"
                  xmlns:x1="urn:x1" xmlns:x2="urn:x2">
            <add sel="p:target/p:item" type="@a:flag">yes</add>
        </p:diff>"#
	)
	.unwrap();

	assert!(result.contains(r#"a:flag="yes""#), "{result}");
	assert!(result.contains(r#"xmlns:t="urn:target""#));
	assert!(result.contains(r#"xmlns:a="urn:attribute""#));
	assert!(!result.contains(r#"x1:"#));
	assert!(!result.contains(r#"x2:"#));
}

#[test]
fn transfers_replacement_names_from_the_patch_xot_into_the_target_xot() {
	let result = apply_patch(
		r#"<t:target xmlns:t="urn:target"><old/></t:target>"#,
		r#"<p:diff xmlns:p="urn:target" xmlns:a="urn:replacement"
                  xmlns:x1="urn:x1" xmlns:x2="urn:x2">
            <replace sel="p:target/old"><a:new/></replace>
        </p:diff>"#
	)
	.unwrap();

	assert!(result.contains("<a:new"), "{result}");
	assert!(result.contains(r#"xmlns:a="urn:replacement""#));
	assert!(!result.contains(r#"x1:"#));
	assert!(!result.contains(r#"x2:"#));
}

#[test]
fn preserves_local_patch_namespace_declarations_when_transferring_content() {
	let result = apply_patch(
		r#"<t:target xmlns:t="urn:target"/>"#,
		r#"<p:diff xmlns:p="urn:target" xmlns:a="urn:outer">
            <add sel="p:target"><a:outer xmlns:b="urn:local"><b:inner/></a:outer></add>
        </p:diff>"#
	)
	.unwrap();

	assert!(result.contains(r#"xmlns:b="urn:local""#), "{result}");
	assert!(result.contains("<b:inner"));
}

#[test]
fn rejects_empty_element_replacement() {
	let result = apply_patch("<root><item/></root>", r#"<replace sel="/root/item"/>"#);

	assert!(matches!(result, Err(Error::Operation(message)) if message.contains("same node type")));
}

#[test]
fn ws_both_is_atomic_when_one_sibling_is_missing() {
	let result = apply_patch("<root>\n<item/></root>", r#"<remove sel="/root/item" ws="both"/>"#);

	assert!(result.is_err());
	let result = apply_patch("<root>\n<item/></root>", r#"<remove sel="/root/item"/>"#).unwrap();
	assert_eq!(result, "<root>\n</root>");
}

#[test]
fn combines_text_siblings_when_removing_an_element() {
	let result = apply_patch("<root>before<item/>after</root>", r#"<remove sel="/root/item"/>"#).unwrap();

	assert_eq!(result, "<root>beforeafter</root>");
}

#[test]
fn validates_directive_scope() {
	let result = apply_patch("<root/>", r#"<replace sel="/root" pos="before"><x/></replace>"#);
	assert!(matches!(result, Err(Error::InvalidPatch(message)) if message.contains("'pos'")));
}

#[test]
fn replaces_and_removes_namespace_declarations() {
	let result = apply_patch(
		r#"<root xmlns:p="urn:old"/>"#,
		r#"<diff>
            <replace sel="/root/namespace::p">urn:new</replace>
            <remove sel="/root/namespace::p"/>
        </diff>"#
	)
	.unwrap();

	assert_eq!(result, "<root/>");
}

#[test]
fn preserves_order_for_multiple_positioned_nodes() {
	let result = apply_patch(
		"<root><target/></root>",
		r#"<diff>
            <add sel="/root/target" pos="before"><a/><b/></add>
            <add sel="/root/target" pos="after"><c/><d/></add>
        </diff>"#
	)
	.unwrap();

	assert_eq!(result, "<root><a/><b/><target/><c/><d/></root>");
}

#[test]
fn supports_default_namespace_selectors_and_qualified_attributes() {
	let result = apply_patch(
		r#"<t:root xmlns:t="urn:test"/>"#,
		r#"<d:diff xmlns:d="urn:test" xmlns="urn:test" xmlns:a="urn:attr">
            <add sel="root" type="@a:kind">value</add>
        </d:diff>"#
	)
	.unwrap();

	assert!(result.contains("a:kind=\"value\""));
	assert!(result.contains("xmlns:a=\"urn:attr\""));
}

#[test]
fn rejects_undeclared_namespace_prefixes() {
	let result = apply_patch("<root/>", r#"<diff><add sel="/root" type="@a:value">x</add></diff>"#);

	assert!(matches!(result, Err(Error::InvalidPatch(message)) if message.contains("undeclared")));
}

#[test]
fn rejects_malformed_attribute_directives() {
	let result = apply_patch("<root/>", r#"<add sel="/root" type="@@value">x</add>"#);

	assert!(matches!(result, Err(Error::Operation(message)) if message.contains("invalid attribute")));
}

#[test]
fn rejects_prohibited_xpath_namespace_functions() {
	let result = apply_patch("<root/>", r#"<remove sel="/root[local-name()='root']"/>"#);

	assert!(matches!(result, Err(Error::InvalidSelector(message)) if message.contains("not allowed")));
}

#[test]
fn replaces_comment_and_processing_instruction_nodes() {
	let result = apply_patch(
		"<root><!--old--><?test old?></root>",
		r#"<diff>
            <replace sel="/root/comment()"><!--new--></replace>
            <replace sel="/root/processing-instruction(&quot;test&quot;)"><?test new?></replace>
        </diff>"#
	)
	.unwrap();

	assert_eq!(result, "<root><!--new--><?test new?></root>");
}

#[test]
fn adds_text_around_an_existing_text_node_without_adjacent_text_nodes() {
	let result = apply_patch(
		"<root><item>one</item></root>",
		r#"<add sel="/root/item/text()" pos="after">two<mark/>three</add>"#
	)
	.unwrap();

	assert_eq!(result, "<root><item>onetwo<mark/>three</item></root>");
}

#[test]
fn supports_append_and_prepend_positions() {
	let result = apply_patch(
		"<root><existing/></root>",
		r#"<diff>
            <add sel="/root" pos="prepend"><first/><second/></add>
            <add sel="/root" pos="append"><third/><fourth/></add>
        </diff>"#
	)
	.unwrap();

	assert_eq!(result, "<root><first/><second/><existing/><third/><fourth/></root>");
}

#[test]
fn adds_a_prefixed_namespace_declaration() {
	let result = apply_patch(
		"<root/>",
		r#"<diff><add sel="/root" type="namespace::p">urn:example</add></diff>"#
	)
	.unwrap();

	assert_eq!(result, r#"<root xmlns:p="urn:example"/>"#);
}

#[test]
fn empty_replacement_preserves_attributes_and_removes_text() {
	let result = apply_patch(
		r#"<root a="value">text</root>"#,
		r#"<diff>
            <replace sel="/root/@a"/>
            <replace sel="/root/text()"/>
        </diff>"#
	)
	.unwrap();

	assert_eq!(result, r#"<root a=""/>"#);
}

#[test]
fn removes_attributes_comments_processing_instructions_and_text() {
	let result = apply_patch(
		r#"<root a="value"><!--comment--><?test data?><item>text</item></root>"#,
		r#"<diff>
            <remove sel="/root/@a"/>
            <remove sel="/root/comment()"/>
            <remove sel="/root/processing-instruction(&quot;test&quot;)"/>
            <remove sel="/root/item/text()"/>
        </diff>"#
	)
	.unwrap();

	assert_eq!(result, "<root><item/></root>");
}

#[test]
fn supports_all_whitespace_modes() {
	let result = apply_patch(
		"<root>\n<before/>\n<target/>\n<after/>\n</root>",
		r#"<diff><remove sel="/root/target" ws="both"/></diff>"#
	)
	.unwrap();

	assert_eq!(result, "<root>\n<before/><after/>\n</root>");

	let result = apply_patch(
		"<root><target/>\n<after/></root>",
		r#"<diff><remove sel="/root/target" ws="after"/></diff>"#
	)
	.unwrap();
	assert_eq!(result, "<root><after/></root>");
}

#[test]
fn rejects_ambiguous_selectors_and_root_element_operations() {
	let result = apply_patch("<root><item/><item/></root>", r#"<remove sel="/root/item"/>"#);
	assert!(matches!(result, Err(Error::InvalidSelector(_))));

	let result = apply_patch("<root/>", r#"<remove sel="/root"/>"#);
	assert!(matches!(result, Err(Error::Operation(_))));

	let result = apply_patch("<root/>", r#"<add sel="/root" pos="before"><sibling/></add>"#);
	assert!(matches!(result, Err(Error::Operation(_))));
}

#[test]
fn applies_operations_sequentially() {
	let result = apply_patch(
		"<root><item id=\"old\"/></root>",
		r#"<diff>
            <replace sel="/root/item"><new id="new"/></replace>
            <add sel="/root/new" type="@seen">yes</add>
        </diff>"#
	)
	.unwrap();

	assert_eq!(result, r#"<root><new id="new" seen="yes"/></root>"#);
}

#[test]
fn merges_whitespace_around_removed_comment_and_processing_instruction() {
	let result = apply_patch(
		"<root>before\n<!--comment-->\nafter<?test data?>end</root>",
		r#"<diff>
            <remove sel="/root/comment()"/>
            <remove sel="/root/processing-instruction(&quot;test&quot;)"/>
        </diff>"#
	)
	.unwrap();

	assert_eq!(result, "<root>before\n\nafterend</root>");
}

#[test]
fn replaces_attribute_values() {
	let result = apply_patch(
		r#"<root old="value"/>"#,
		r#"<replace sel="/root/@old">new value</replace>"#
	)
	.unwrap();

	assert_eq!(result, r#"<root old="new value"/>"#);
}

#[test]
fn adds_comments_and_processing_instructions_before_a_node() {
	let result = apply_patch(
		"<root><item/></root>",
		r#"<diff>
            <add sel="/root/item" pos="before"><!-- comment --><?test data?></add>
        </diff>"#
	)
	.unwrap();

	assert_eq!(result, "<root><!-- comment --><?test data?><item/></root>");
}

#[test]
fn coalesces_text_when_adding_text_after_text() {
	let result = apply_patch(
		"<root>before</root>",
		r#"<add sel="/root/text()" pos="after">after</add>"#
	)
	.unwrap();

	assert_eq!(result, "<root>beforeafter</root>");
}

#[test]
fn coalesces_text_when_adding_text_before_text() {
	let result = apply_patch(
		"<root>after</root>",
		r#"<add sel="/root/text()" pos="before">before</add>"#
	)
	.unwrap();

	assert_eq!(result, "<root>beforeafter</root>");
}

#[test]
fn supports_wildcard_string_and_positional_selectors() {
	let result = apply_patch(
		"<root><item>first</item><other>skip</other><item>second</item></root>",
		r#"<replace sel="/*/item[2][.='second']/text()[1]">updated</replace>"#
	)
	.unwrap();

	assert_eq!(
		result,
		"<root><item>first</item><other>skip</other><item>updated</item></root>"
	);
}

#[test]
fn unqualified_selectors_do_not_match_namespaced_elements() {
	let result = apply_patch(
		r#"<root xmlns="urn:test"><item/></root>"#,
		r#"<remove sel="/root/item"/>"#
	);

	assert!(matches!(result, Err(Error::InvalidSelector(_))));
}

#[test]
fn patches_comments_and_processing_instructions_at_the_document_root() {
	let result = apply_patch(
		"<?before data?><root/><?after data?>",
		r#"<diff>
            <replace sel="/processing-instruction(&quot;before&quot;)"><?before changed?></replace>
            <remove sel="/processing-instruction(&quot;after&quot;)"/>
        </diff>"#
	)
	.unwrap();

	assert_eq!(result, "<?before changed?><root/>");
}

#[test]
fn rejects_removing_a_namespace_used_by_an_element() {
	let result = apply_patch(
		r#"<root xmlns:p="urn:test"><p:item/></root>"#,
		r#"<remove sel="/root/namespace::p"/>"#
	);

	assert!(result.is_err());
}

#[test]
fn example_rfc7351() {
	let result = apply_patch(
		r#"<?xml version="1.0" encoding="UTF-8"?>
   <doc>
       <note>This is a sample document</note>
   </doc>"#,
		r#"<?xml version="1.0" encoding="UTF-8"?>
<p:patch xmlns:p="urn:ietf:rfc:7351">
    <p:add sel="doc"><foo id="ert4773">This is a new child</foo></p:add>
</p:patch>"#
	)
	.unwrap();

	assert_eq!(
		result,
		r#"<doc>
       <note>This is a sample document</note>
   <foo id="ert4773">This is a new child</foo></doc>"#
	);
}

#[test]
fn removes_unused_patch_namespace_declarations_from_added_content() {
	let result = apply_patch(
		"<root xmlns:a=\"urn:a\"/>",
		r#"<p:patch xmlns:p="urn:ietf:rfc:7351">
            <p:add sel="/root"><item/></p:add>
        </p:patch>"#
	)
	.unwrap();

	assert_eq!(result, "<root xmlns:a=\"urn:a\"><item/></root>");
}

#[test]
fn keeps_namespace_declarations_used_by_descendants_and_attributes() {
	let result = apply_patch(
		"<root/>",
		r#"<p:patch xmlns:p="urn:ietf:rfc:7351" xmlns:a="urn:attribute">
            <p:add sel="/root">
                <item xmlns:b="urn:item" a:flag="yes"><b:child/></item>
            </p:add>
        </p:patch>"#
	)
	.unwrap();

	assert!(result.contains(r#"xmlns:a="urn:attribute""#), "{result}");
	assert!(result.contains(r#"xmlns:b="urn:item""#), "{result}");
	assert!(!result.contains(r#"xmlns:p="urn:ietf:rfc:7351""#), "{result}");
}
