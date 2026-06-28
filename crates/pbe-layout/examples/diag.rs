use cap_css_parse::Stylesheet;
use cap_html_parse::parse_html;
use cap_style_cascade::StyledDom;

fn main() {
    let dom = parse_html("<html><body><div class=a></div><div class=b></div></body></html>");
    let sheet = Stylesheet::parse_author(
        "html{display:block} body{display:block} div{display:block} body{margin:8px} \
         .a{width:100px;height:50px} .b{width:100px;height:60px}",
    );
    let styled = StyledDom::new(dom, &[sheet]);
    for (id, node) in styled.tree.iter() {
        let tag = if let Some(t) = node.tag() {
            t.to_string()
        } else if node.is_text() {
            "#text".to_string()
        } else {
            "#doc".to_string()
        };
        let disp = styled
            .style(id)
            .map(|s| format!("{:?}", s.layout.display))
            .unwrap_or_else(|| "-".into());
        println!(
            "node {:>2} {:<8} display={:<7} children={}",
            id.0,
            tag,
            disp,
            node.children().len()
        );
    }
    let r = pbe_layout::layout(&styled, 800.0, 600.0);
    println!("--- layout boxes ---");
    for (id, node) in styled.tree.iter() {
        if let Some(b) = r.bounds(id) {
            println!(
                "node {:>2} {:<8} x={:.0} y={:.0} w={:.0} h={:.0}",
                id.0,
                node.tag().unwrap_or("#text"),
                b.origin.x.0,
                b.origin.y.0,
                b.size.width.0,
                b.size.height.0
            );
        }
    }
}
