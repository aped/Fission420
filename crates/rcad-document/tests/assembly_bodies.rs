//! Run 7 body ownership with an active component (review #16): ordinary new
//! bodies join the active component; unowned (display) bodies never do;
//! derived bodies replace their source in the source's component.
use rcad_document::Document;

#[test]
fn active_unowned_and_derived_bodies() {
    let mut d = Document::new();
    let a = d.add_body("A");
    let c = d.assembly.new_component("C".into(), vec![a], String::new());
    d.assembly.active = Some(c);
    let b = d.add_body("B");
    assert_eq!(d.assembly.component(c).unwrap().bodies, vec![a, b]);
    let flat = d.add_body_unowned("flat");
    assert!(d.assembly.component_of(flat).is_none());
    assert_eq!(d.assembly.active, Some(c));
    // Derived from a component body: takes its place (source leaves).
    let thick = d.add_derived_body("thick", a);
    assert_eq!(d.assembly.component(c).unwrap().bodies, vec![thick, b]);
    // Derived from a loose body: unowned even while C is active.
    let loose = {
        d.assembly.active = None;
        let x = d.add_body("loose");
        d.assembly.active = Some(c);
        x
    };
    let fixed = d.add_derived_body("fixed", loose);
    assert!(d.assembly.component_of(fixed).is_none());
}
