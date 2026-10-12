use designcraft_doc::StrokeAlign;

use super::*;

fn fixture(applied: &str, local: &str) -> Document {
    let styles = r#"<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
      <RootCharacterStyleGroup>
        <CharacterStyle Self="CharacterStyle/$ID/[No character style]" Name="$ID/[No character style]"/>
        <CharacterStyle Self="CharacterStyle/Behind" Name="Behind" StrokeAlignment="OutsideAlignment"/>
      </RootCharacterStyleGroup>
    </idPkg:Styles>"#;
    let story = format!(
        r#"<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"><Story Self="s"><ParagraphStyleRange><CharacterStyleRange AppliedCharacterStyle="CharacterStyle/{applied}" StrokeColor="Color/Black" StrokeWeight="2" {local}><Content>Ink</Content></CharacterStyleRange></ParagraphStyleRange></Story></idPkg:Story>"#
    );
    let map = r#"<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"><idPkg:Styles src="Resources/Styles.xml"/><idPkg:Story src="Stories/s.xml"/><Layer Self="L" Name="Art"/><idPkg:Spread src="Spreads/s.xml"/></Document>"#;
    let spread = r#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"><Spread Self="sp"><Page Self="p" GeometricBounds="0 0 240 240"/><TextFrame Self="frame" ParentStory="s" ItemLayer="L" GeometricBounds="0 0 200 200"><Properties><PathGeometry><GeometryPathType><PathPointArray><PathPointType Anchor="0 0"/><PathPointType Anchor="200 0"/><PathPointType Anchor="200 200"/><PathPointType Anchor="0 200"/></PathPointArray></GeometryPathType></PathGeometry></Properties></TextFrame></Spread></idPkg:Spread>"#;
    import_idml(&zip_files(&[("designmap.xml", map), ("Resources/Styles.xml", styles), ("Stories/s.xml", &story), ("Spreads/s.xml", spread)]))
        .unwrap()
}

fn alignment(d: &Document) -> StrokeAlign {
    let story = d.stories.values().next().unwrap();
    let (_, para) = d.styles.resolve_para(&story.paras[0]);
    d.styles.resolve_char(&para, story.char_format_at(0)).stroke_alignment
}

#[test]
fn text_stroke_alignment_is_read_locally_and_from_character_styles_through_roundtrip() {
    for (applied, local, expected) in [
        ("$ID/[No character style]", r#"StrokeAlignment="OutsideAlignment""#, StrokeAlign::Outside),
        ("$ID/[No character style]", r#"StrokeAlignment="CenterAlignment""#, StrokeAlign::Center),
        ("$ID/[No character style]", "", StrokeAlign::Center),
        ("Behind", "", StrokeAlign::Outside),
        ("Behind", r#"StrokeAlignment="CenterAlignment""#, StrokeAlign::Center),
    ] {
        let imported = fixture(applied, local);
        assert_eq!(alignment(&imported), expected, "{applied} {local}");
        assert_eq!(alignment(&import_idml(&export_idml(&imported)).unwrap()), expected, "round trip: {applied} {local}");
    }
}
