//! Canvas element extraction and 2D context drawing from inline scripts.

use super::*;

#[test]
fn extracts_canvas_sizes_without_fallback_content() {
    let page = parse_html(
        "https://example.test/",
        concat!(
            "<canvas>fallback</canvas>",
            "<canvas width=' +12px' height='0'>hidden</canvas>",
            "<canvas width='-1' height='3'></canvas>",
            "<canvas width='16385' height='1'>not shown</canvas>",
        ),
    );

    assert_eq!(page.blocks.len(), 3);

    let sizes: Vec<_> = page
        .blocks
        .iter()
        .map(|block| match &block.content {
            BlockContent::Canvas(canvas) => (
                canvas.canvas.surface().width(),
                canvas.canvas.surface().height(),
            ),
            _ => panic!("canvas block expected"),
        })
        .collect();

    assert_eq!(sizes, [(300, 150), (12, 0), (300, 3)]);

    assert_eq!(parse_canvas_dimension("-0junk"), Some(0));
    assert_eq!(parse_canvas_dimension("+no"), None);
    assert_eq!(parse_canvas_dimension("-1"), None);
    assert_eq!(
        parse_canvas_dimension("9999999999999999999999999999999999"),
        Some(usize::MAX)
    );

    let layout = crate::page_layout::PageLayout::build(&page, 128, None);
    let rects: Vec<_> = layout.block_rects().collect();

    assert_eq!((rects[0].width, rects[0].height), (300, 150));
    assert_eq!((rects[1].width, rects[1].height), (12, 0));
}

#[test]
fn canvas_blocks_preserve_parsed_dom_identity() {
    let page = parse_html(
        "https://example.test/",
        concat!(
            "<div>text<!-- comment -->",
            "<canvas id='dup' width='2' height='2'></canvas>",
            "<canvas id='dup' width='2' height='2'></canvas>",
            "</div>",
            "<script>document.getElementById('dup').id = 'new';</script>"
        ),
    );

    let ids: Vec<_> = page
        .blocks
        .iter()
        .filter_map(|block| match &block.content {
            BlockContent::Canvas(canvas) => Some(canvas.node_id),
            _ => None,
        })
        .collect();

    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);

    assert_eq!(page.document.get_element_by_id("new"), Some(ids[0]));
    assert_eq!(page.document.get_element_by_id("dup"), Some(ids[1]));

    let index = page
        .blocks
        .iter()
        .position(|block| {
            matches!(
                &block.content,
                BlockContent::Canvas(canvas) if canvas.node_id == ids[0]
            )
        })
        .unwrap();

    assert!(
        page.describe_block_sources(index)
            .unwrap()
            .contains(&format!("DOM node_id={} connected=true", ids[0]))
    );

    let page = parse_html(
        "https://example.test/",
        concat!(
            "<canvas id='c' width='2' height='4'></canvas>",
            "<script>",
            "document.getElementById('c').setAttribute('width', '7');",
            "document.getElementById('c').removeAttribute('height');",
            "</script>",
        ),
    );

    let BlockContent::Canvas(canvas) = &page.blocks[0].content else {
        panic!("canvas expected");
    };

    assert_eq!(
        (
            canvas.canvas.surface().width(),
            canvas.canvas.surface().height()
        ),
        (7, 150)
    );

    let page = parse_html(
        "https://example.test/",
        concat!(
            "<div id='parent'><canvas id='dead'></canvas></div>",
            "<script>",
            "document.getElementById('parent').removeChild(",
            "document.getElementById('dead'));",
            "</script>",
        ),
    );

    assert!(
        !page
            .blocks
            .iter()
            .any(|block| matches!(block.content, BlockContent::Canvas(_)))
    );
}

#[test]
fn draws_canvas_from_inline_script() {
    use crate::graphics::raster::Color;

    let page = parse_html(
        "https://example.test/",
        r#"
                <canvas id='picture' width='4' height='3'></canvas>
                <script>
                    let ctx = document.getElementById('picture').getContext('2d');
                    ctx.fillStyle = 'red';
                    ctx.fillRect(1, 1, 2, 1);
                    ctx.fillStyle = 'not-a-color';
                    ctx.fillRect(0, 0, 1, 1);
                </script>
            "#,
    );

    let BlockContent::Canvas(canvas) = &page.blocks[0].content else {
        panic!("canvas block expected");
    };

    let image = canvas.canvas.surface();

    assert_eq!(image.pixel(1, 1), Some(Color::rgba(255, 0, 0, 255)));
    assert_eq!(image.pixel(0, 0), Some(Color::rgba(255, 0, 0, 255)));
    assert_eq!(image.pixel(3, 2), Some(Color::TRANSPARENT));
}

#[test]
fn clears_and_resets_canvas_from_inline_script() {
    use crate::graphics::raster::Color;

    let page = parse_html(
        "https://example.test/",
        r#"
                <canvas id='picture' width='4' height='3'></canvas>
                <script>
                    let canvas = document.getElementById('picture');
                    let ctx = canvas.getContext('2d');

                    ctx.fillStyle = 'red';
                    ctx.fillRect(0, 0, 4, 3);

                    canvas.width = canvas.width;
                    ctx.fillRect(0, 0, 1, 1);

                    ctx.fillStyle = 'blue';
                    canvas.setAttribute('height', '3');
                    ctx.fillRect(1, 0, 1, 1);

                    ctx.fillStyle = 'red';
                    ctx.fillRect(0, 1, 4, 1);
                    ctx.clearRect(1, 1, 2, 1);
                </script>
            "#,
    );

    let BlockContent::Canvas(canvas) = &page.blocks[0].content else {
        panic!("canvas block expected");
    };

    let image = canvas.canvas.surface();

    assert_eq!(image.pixel(0, 0), Some(Color::TRANSPARENT));
    assert_eq!(image.pixel(1, 0), Some(Color::BLACK));
    assert_eq!(image.pixel(0, 1), Some(Color::rgba(255, 0, 0, 255)));
    assert_eq!(image.pixel(1, 1), Some(Color::TRANSPARENT));
    assert_eq!(image.pixel(3, 1), Some(Color::rgba(255, 0, 0, 255)));
}

#[test]
fn fills_canvas_paths_from_inline_script() {
    use crate::graphics::raster::Color;

    let page = parse_html(
        "https://example.test/",
        r#"
                <canvas id='picture' width='8' height='8'></canvas>
                <script>
                    let canvas = document.getElementById('picture');
                    let ctx = canvas.getContext('2d');

                    ctx.fillStyle = 'red';
                    ctx.beginPath();
                    ctx.moveTo(0, 0);
                    ctx.lineTo(8, 0);
                    ctx.lineTo(0, 8);
                    ctx.fill();

                    ctx.beginPath();
                    ctx.fillStyle = 'blue';
                    ctx.moveTo(6, 6);
                    ctx.lineTo(8, 6);
                    ctx.lineTo(8, 8);
                    ctx.lineTo(6, 8);
                    ctx.closePath();
                    ctx.fill('evenodd');
                </script>
            "#,
    );

    let BlockContent::Canvas(canvas) = &page.blocks[0].content else {
        panic!("canvas block expected");
    };

    let image = canvas.canvas.surface();

    assert_eq!(image.pixel(1, 1), Some(Color::rgba(255, 0, 0, 255)));
    assert_eq!(image.pixel(6, 6), Some(Color::rgba(0, 0, 255, 255)));
    assert_eq!(image.pixel(5, 5), Some(Color::TRANSPARENT));
}

#[test]
fn fills_canvas_bezier_curves_from_inline_script() {
    use crate::graphics::raster::Color;

    let page = parse_html(
        "https://example.test/",
        r#"
                <canvas id='picture' width='20' height='10'></canvas>
                <script>
                    let ctx = document.getElementById('picture').getContext('2d');

                    ctx.fillStyle = 'red';
                    ctx.beginPath();
                    ctx.moveTo(0, 8);
                    ctx.quadraticCurveTo(5, -2, 10, 8);
                    ctx.closePath();
                    ctx.fill();

                    ctx.fillStyle = 'blue';
                    ctx.beginPath();
                    ctx.moveTo(10, 8);
                    ctx.bezierCurveTo(10, -2, 20, -2, 20, 8);
                    ctx.closePath();
                    ctx.fill();
                </script>
            "#,
    );

    let BlockContent::Canvas(canvas) = &page.blocks[0].content else {
        panic!("canvas block expected");
    };

    let image = canvas.canvas.surface();

    assert_eq!(image.pixel(4, 6), Some(Color::rgba(255, 0, 0, 255)));
    assert_eq!(image.pixel(14, 6), Some(Color::rgba(0, 0, 255, 255)));
    assert_eq!(image.pixel(0, 0), Some(Color::TRANSPARENT));
}

#[test]
fn strokes_canvas_paths_from_inline_script() {
    use crate::graphics::raster::Color;

    let page = parse_html(
        "https://example.test/",
        r#"
                <canvas id='picture' width='10' height='10'></canvas>
                <script>
                    let canvas = document.getElementById('picture');
                    let ctx = canvas.getContext('2d');

                    ctx.strokeStyle = 'red';
                    ctx.lineWidth = 4;
                    canvas.width = canvas.width;

                    ctx.beginPath();
                    ctx.moveTo(1, 2.5);
                    ctx.lineTo(9, 2.5);
                    ctx.stroke();

                    ctx.strokeStyle = 'blue';
                    ctx.lineWidth = 2;
                    ctx.lineWidth = 0;

                    ctx.beginPath();
                    ctx.moveTo(1, 7);
                    ctx.lineTo(9, 7);
                    ctx.stroke();
                </script>
            "#,
    );

    let BlockContent::Canvas(canvas) = &page.blocks[0].content else {
        panic!("canvas block expected");
    };

    let image = canvas.canvas.surface();

    assert_eq!(image.pixel(4, 2), Some(Color::BLACK));
    assert_eq!(image.pixel(4, 3), Some(Color::TRANSPARENT));

    assert_eq!(image.pixel(4, 6), Some(Color::rgba(0, 0, 255, 255)));
    assert_eq!(image.pixel(4, 7), Some(Color::rgba(0, 0, 255, 255)));
    assert_eq!(image.pixel(4, 8), Some(Color::TRANSPARENT));
}

#[test]
fn canvas_stroke_caps_joins_and_reset() {
    use crate::graphics::raster::Color;

    let page = parse_html(
        "https://example.test/",
        r#"
                <canvas id='cap' width='14' height='14'></canvas>
                <canvas id='join' width='14' height='14'></canvas>
                <canvas id='miter' width='14' height='14'></canvas>
                <canvas id='limited' width='14' height='14'></canvas>

                <script>
                    let cap = document.getElementById('cap');
                    let ctx = cap.getContext('2d');

                    ctx.lineWidth = 4;
                    ctx.lineCap = 'round';
                    ctx.lineJoin = 'bevel';
                    ctx.miterLimit = 3;

                    ctx.lineCap = 'ROUND';
                    ctx.lineJoin = 'invalid';
                    ctx.miterLimit = 0;

                    document.title =
                        ctx.lineCap + '/' + ctx.lineJoin + '/' + ctx.miterLimit;

                    ctx.strokeStyle = 'red';
                    ctx.beginPath();
                    ctx.moveTo(3, 3);
                    ctx.lineTo(10, 3);
                    ctx.stroke();

                    ctx.lineCap = 'butt';
                    ctx.strokeStyle = 'blue';
                    ctx.beginPath();
                    ctx.moveTo(3, 10);
                    ctx.lineTo(10, 10);
                    ctx.stroke();

                    let other = document.getElementById('join');
                    let c = other.getContext('2d');

                    c.lineWidth = 4;
                    c.lineJoin = 'bevel';
                    c.lineCap = 'square';
                    c.miterLimit = 3;
                    other.width = other.width;

                    document.title = document.title + '|'
                        + c.lineCap + '/' + c.lineJoin + '/' + c.miterLimit;

                    c.lineWidth = 4;
                    c.lineJoin = 'bevel';
                    c.beginPath();
                    c.moveTo(2, 10);
                    c.lineTo(7, 3);
                    c.lineTo(12, 10);
                    c.stroke();

                    let m = document.getElementById('miter').getContext('2d');
                    m.lineWidth = 4;
                    m.beginPath();
                    m.moveTo(2, 10);
                    m.lineTo(7, 3);
                    m.lineTo(12, 10);
                    m.stroke();

                    let l = document.getElementById('limited').getContext('2d');
                    l.lineWidth = 4;
                    l.miterLimit = 1;
                    l.beginPath();
                    l.moveTo(2, 10);
                    l.lineTo(7, 3);
                    l.lineTo(12, 10);
                    l.stroke();
                </script>
            "#,
    );

    assert_eq!(page.title, "round/bevel/3|butt/miter/10");

    let BlockContent::Canvas(canvas) = &page.blocks[0].content else {
        panic!("canvas expected");
    };

    let image = canvas.canvas.surface();

    assert!(image.pixel(1, 2).unwrap().alpha > 0);
    assert_eq!(image.pixel(1, 9), Some(Color::TRANSPARENT));
    assert_eq!(image.pixel(4, 9), Some(Color::rgba(0, 0, 255, 255)));

    let BlockContent::Canvas(bevel) = &page.blocks[1].content else {
        panic!("bevel canvas expected");
    };

    let BlockContent::Canvas(miter) = &page.blocks[2].content else {
        panic!("miter canvas expected");
    };

    let BlockContent::Canvas(limited) = &page.blocks[3].content else {
        panic!("limited canvas expected");
    };

    assert_eq!(bevel.canvas.surface().pixel(6, 0), Some(Color::TRANSPARENT));
    assert!(miter.canvas.surface().pixel(6, 0).unwrap().alpha > 0);
    assert_eq!(miter.canvas.surface().pixel(6, 2), Some(Color::BLACK));
    assert_eq!(
        limited.canvas.surface().pixel(6, 0),
        Some(Color::TRANSPARENT)
    );
}

#[test]
fn draws_and_resets_canvas_dashes_from_inline_script() {
    use crate::graphics::raster::Color;

    let page = parse_html(
        "https://example.test/",
        r#"
                <canvas id='a' width='16' height='6'></canvas>
                <canvas id='b' width='1' height='1'></canvas>

                <script>
                    let a = document.getElementById('a');
                    let ctx = a.getContext('2d');

                    ctx.setLineDash([3, 2, 1]);

                    let copy = ctx.getLineDash();
                    copy[0] = 88;

                    ctx.setLineDash([-1, 2]);
                    ctx.lineDashOffset = 2;

                    document.title = ctx.getLineDash()[0] + '/'
                        + ctx.getLineDash().length + '/'
                        + ctx.lineDashOffset;

                    ctx.beginPath();
                    ctx.moveTo(1, 2.5);
                    ctx.lineTo(15, 2.5);
                    ctx.stroke();

                    ctx.setLineDash([0, 2]);
                    document.title = document.title + '/' + ctx.getLineDash()[0];

                    let b = document.getElementById('b');
                    let other = b.getContext('2d');

                    other.setLineDash([2, 1]);
                    other.lineDashOffset = -2;
                    b.width = b.width;

                    document.title = document.title + '/'
                        + other.getLineDash().length + '/'
                        + other.lineDashOffset;
                </script>
            "#,
    );

    assert_eq!(page.title, "3/6/2/0/0/0");

    let BlockContent::Canvas(canvas) = &page.blocks[0].content else {
        panic!("canvas expected");
    };

    let image = canvas.canvas.surface();

    assert_eq!(image.pixel(1, 2), Some(Color::BLACK));
    assert_eq!(image.pixel(2, 2), Some(Color::TRANSPARENT));
    assert_eq!(image.pixel(3, 2), Some(Color::TRANSPARENT));
    assert_eq!(image.pixel(4, 2), Some(Color::BLACK));
}

#[test]
fn draws_zero_length_canvas_dash_segments() {
    use crate::graphics::raster::Color;

    let page = parse_html(
        "https://example.test/",
        r#"
                <canvas id='a' width='28' height='28'></canvas>
                <canvas id='b' width='24' height='24'></canvas>
                <canvas id='closed' width='14' height='14'></canvas>

                <script>
                    let ctx = document.getElementById('a').getContext('2d');

                    ctx.lineWidth = 4;
                    ctx.lineCap = 'round';
                    ctx.setLineDash([0, 6]);

                    ctx.beginPath();
                    ctx.moveTo(3, 4);
                    ctx.lineTo(21, 4);
                    ctx.stroke();

                    ctx.lineDashOffset = -3;
                    ctx.beginPath();
                    ctx.moveTo(3, 11);
                    ctx.lineTo(21, 11);
                    ctx.stroke();

                    ctx.lineDashOffset = 0;
                    ctx.lineCap = 'butt';
                    ctx.setLineDash([4, 0]);
                    ctx.beginPath();
                    ctx.moveTo(3, 18);
                    ctx.lineTo(21, 18);
                    ctx.stroke();

                    ctx.setLineDash([0, 0]);
                    ctx.beginPath();
                    ctx.moveTo(3, 24);
                    ctx.lineTo(21, 24);
                    ctx.stroke();

                    let square = document.getElementById('b').getContext('2d');
                    square.lineWidth = 4;
                    square.lineCap = 'square';
                    square.setLineDash([0, 8]);
                    square.beginPath();
                    square.moveTo(4, 4);
                    square.lineTo(20, 20);
                    square.stroke();

                    let closed = document.getElementById('closed').getContext('2d');
                    closed.lineWidth = 4;
                    closed.beginPath();
                    closed.moveTo(2, 10);
                    closed.lineTo(7, 3);
                    closed.lineTo(12, 10);
                    closed.closePath();
                    closed.stroke();
                </script>
            "#,
    );

    let BlockContent::Canvas(a) = &page.blocks[0].content else {
        panic!("canvas expected");
    };

    let image = a.canvas.surface();

    assert_eq!(image.pixel(3, 3), Some(Color::BLACK));
    assert_eq!(image.pixel(6, 3), Some(Color::TRANSPARENT));
    assert_eq!(image.pixel(9, 3), Some(Color::BLACK));
    assert_eq!(image.pixel(21, 3), Some(Color::BLACK));

    assert_eq!(image.pixel(3, 10), Some(Color::TRANSPARENT));
    assert_eq!(image.pixel(6, 10), Some(Color::BLACK));

    assert_eq!(image.pixel(7, 17), Some(Color::BLACK));
    assert_eq!(image.pixel(7, 23), Some(Color::BLACK));

    let BlockContent::Canvas(b) = &page.blocks[1].content else {
        panic!("square canvas expected");
    };

    assert!(b.canvas.surface().pixel(5, 5).unwrap().alpha > 0);

    let BlockContent::Canvas(closed) = &page.blocks[2].content else {
        panic!("closed canvas expected");
    };

    assert!(closed.canvas.surface().pixel(6, 0).unwrap().alpha > 0);
}

#[test]
fn connects_canvas_dashes_across_closed_path_seam() {
    use crate::graphics::raster::Color;

    let page = parse_html(
        "https://example.test/",
        r#"
                <canvas id='wrap' width='16' height='16'></canvas>
                <canvas id='full' width='16' height='16'></canvas>
                <canvas id='gap' width='16' height='16'></canvas>
                <canvas id='zero' width='16' height='16'></canvas>

                <script>
                    function draw(ctx) {
                        ctx.lineWidth = 4;

                        ctx.beginPath();
                        ctx.moveTo(3, 3);
                        ctx.lineTo(13, 3);
                        ctx.lineTo(13, 13);
                        ctx.lineTo(3, 13);
                        ctx.closePath();
                        ctx.stroke();
                    }

                    let a = document.getElementById('wrap').getContext('2d');
                    a.setLineDash([30, 10]);
                    a.lineDashOffset = 5;
                    draw(a);

                    let b = document.getElementById('full').getContext('2d');
                    b.setLineDash([100, 10]);
                    b.lineDashOffset = 5;
                    draw(b);

                    let c = document.getElementById('gap').getContext('2d');
                    c.setLineDash([10, 50]);
                    c.lineDashOffset = 5;
                    draw(c);

                    let d = document.getElementById('zero').getContext('2d');
                    d.setLineDash([40, 0]);
                    draw(d);
                </script>
            "#,
    );

    let pixel = |index: usize| match &page.blocks[index].content {
        BlockContent::Canvas(canvas) => canvas.canvas.surface().pixel(1, 1).unwrap(),

        _ => panic!("canvas expected"),
    };

    assert!(pixel(0).alpha > 0);
    assert!(pixel(1).alpha > 0);

    assert_eq!(pixel(2), Color::TRANSPARENT);
    assert_eq!(pixel(3), Color::TRANSPARENT);
}

#[test]
fn saves_and_restores_canvas_drawing_state() {
    use crate::graphics::raster::Color;

    let page = parse_html(
        "https://example.test/",
        r#"
                <canvas id='a' width='10' height='10'></canvas>
                <canvas id='b' width='2' height='2'></canvas>

                <script>
                    let canvas = document.getElementById('a');
                    let ctx = canvas.getContext('2d');

                    ctx.fillStyle = 'red';
                    ctx.fillRect(0, 0, 1, 1);

                    ctx.fillStyle = 'blue';
                    ctx.strokeStyle = 'blue';
                    ctx.lineWidth = 2;
                    ctx.lineCap = 'round';
                    ctx.lineJoin = 'bevel';
                    ctx.miterLimit = 3;
                    ctx.setLineDash([2, 1]);
                    ctx.lineDashOffset = 2;
                    ctx.save();

                    ctx.fillStyle = 'green';
                    ctx.setLineDash([4, 3]);
                    ctx.lineDashOffset = 9;
                    ctx.save();

                    ctx.fillStyle = 'red';
                    ctx.strokeStyle = 'red';
                    ctx.lineWidth = 9;
                    ctx.lineCap = 'square';
                    ctx.lineJoin = 'miter';
                    ctx.miterLimit = 8;
                    ctx.setLineDash([8, 1]);
                    ctx.lineDashOffset = 7;

                    ctx.beginPath();
                    ctx.moveTo(2, 2);
                    ctx.lineTo(8, 2);
                    ctx.lineTo(2, 8);
                    ctx.closePath();

                    ctx.restore();

                    let ok = ctx.fillStyle === '#008000'
                        && ctx.getLineDash()[0] === 4
                        && ctx.lineDashOffset === 9;

                    ctx.restore();
                    ctx.restore();

                    ok = ok
                        && ctx.fillStyle === '#0000ff'
                        && ctx.strokeStyle === '#0000ff'
                        && ctx.lineWidth === 2
                        && ctx.lineCap === 'round'
                        && ctx.lineJoin === 'bevel'
                        && ctx.miterLimit === 3
                        && ctx.getLineDash()[0] === 2
                        && ctx.lineDashOffset === 2;

                    ctx.fill();

                    let otherCanvas = document.getElementById('b');
                    let other = otherCanvas.getContext('2d');

                    other.fillStyle = 'red';
                    other.setLineDash([3, 1]);
                    other.save();

                    otherCanvas.width = otherCanvas.width;
                    other.restore();

                    ok = ok
                        && other.fillStyle === '#000000'
                        && other.getLineDash().length === 0;

                    other.fillRect(0, 0, 1, 1);

                    document.title = ok;
                </script>
            "#,
    );

    assert_eq!(page.title, "true");

    let BlockContent::Canvas(a) = &page.blocks[0].content else {
        panic!("canvas expected");
    };

    assert_eq!(
        a.canvas.surface().pixel(0, 0),
        Some(Color::rgba(255, 0, 0, 255))
    );

    assert_eq!(
        a.canvas.surface().pixel(3, 3),
        Some(Color::rgba(0, 0, 255, 255))
    );

    let BlockContent::Canvas(b) = &page.blocks[1].content else {
        panic!("canvas expected");
    };

    assert_eq!(b.canvas.surface().pixel(0, 0), Some(Color::BLACK));
}

#[test]
fn canvas_translation_affects_rectangles_and_new_path_points() {
    use crate::graphics::raster::Color;

    let page = parse_html(
        "https://example.test/",
        r#"
                <canvas id='a' width='12' height='10'></canvas>
                <canvas id='b' width='4' height='4'></canvas>

                <script>
                    let ctx = document.getElementById('a').getContext('2d');

                    ctx.translate(3, 2);
                    ctx.fillStyle = 'blue';
                    ctx.fillRect(0, 0, 2, 2);

                    ctx.save();
                    ctx.translate(2, 0);

                    ctx.beginPath();
                    ctx.moveTo(0, 0);
                    ctx.lineTo(4, 0);
                    ctx.lineTo(0, 4);
                    ctx.closePath();

                    ctx.restore();
                    ctx.fill();

                    ctx.clearRect(1, 0, 1, 1);
                    ctx.fillRect(0, 4, 1, 1);

                    let canvas = document.getElementById('b');
                    let other = canvas.getContext('2d');

                    other.translate(2, 1);
                    other.save();

                    canvas.width = canvas.width;
                    other.restore();
                    other.fillRect(0, 0, 1, 1);
                </script>
            "#,
    );

    let BlockContent::Canvas(a) = &page.blocks[0].content else {
        panic!("canvas expected");
    };

    let surface = a.canvas.surface();

    assert_eq!(surface.pixel(3, 2), Some(Color::rgba(0, 0, 255, 255)));
    assert_eq!(surface.pixel(4, 2), Some(Color::TRANSPARENT));
    assert_eq!(surface.pixel(5, 2), Some(Color::rgba(0, 0, 255, 255)));
    assert_eq!(surface.pixel(3, 6), Some(Color::rgba(0, 0, 255, 255)));
    assert_eq!(surface.pixel(0, 0), Some(Color::TRANSPARENT));

    let BlockContent::Canvas(b) = &page.blocks[1].content else {
        panic!("canvas expected");
    };

    assert_eq!(b.canvas.surface().pixel(0, 0), Some(Color::BLACK));
    assert_eq!(b.canvas.surface().pixel(2, 1), Some(Color::TRANSPARENT));
}

#[test]
fn canvas_rotates_and_clears_rectangles() {
    use crate::graphics::raster::Color;

    let page = parse_html(
        "https://example.test/",
        r#"
                <canvas id='a' width='12' height='10'></canvas>

                <script>
                    let ctx = document.getElementById('a').getContext('2d');

                    ctx.fillStyle = 'blue';
                    ctx.fillRect(0, 0, 12, 10);

                    ctx.save();
                    ctx.translate(8, 2);
                    ctx.rotate(1.5707963267948966);

                    ctx.fillStyle = 'red';
                    ctx.fillRect(0, 0, 4, 2);
                    ctx.clearRect(1, 0, 2, 1);

                    ctx.restore();
                    ctx.fillStyle = 'green';
                    ctx.fillRect(0, 0, 1, 1);
                </script>
            "#,
    );

    let BlockContent::Canvas(a) = &page.blocks[0].content else {
        panic!("canvas expected");
    };

    let surface = a.canvas.surface();

    assert_eq!(surface.pixel(0, 0), Some(Color::rgba(0, 128, 0, 255)));
    assert_eq!(surface.pixel(7, 2), Some(Color::rgba(255, 0, 0, 255)));
    assert_eq!(surface.pixel(6, 3), Some(Color::rgba(255, 0, 0, 255)));
    assert_eq!(surface.pixel(7, 3), Some(Color::TRANSPARENT));
    assert_eq!(surface.pixel(5, 3), Some(Color::rgba(0, 0, 255, 255)));
}

#[test]
fn scales_canvas_rectangles_and_stroke_path() {
    use crate::graphics::raster::Color;

    let page = parse_html(
        "https://example.test/",
        r#"
                <canvas id='a' width='20' height='20'></canvas>
                <canvas id='b' width='20' height='20'></canvas>

                <script>
                    let a = document.getElementById('a').getContext('2d');

                    a.scale(2, 3);
                    a.fillStyle = 'blue';
                    a.fillRect(1, 1, 2, 2);
                    a.clearRect(1, 1, 1, 1);

                    a.strokeStyle = 'red';
                    a.lineWidth = 2;
                    a.beginPath();
                    a.moveTo(1, 3);
                    a.lineTo(7, 3);
                    a.stroke();

                    let b = document.getElementById('b').getContext('2d');

                    b.strokeStyle = 'red';
                    b.lineWidth = 2;
                    b.beginPath();
                    b.moveTo(1, 3);
                    b.lineTo(7, 3);

                    b.scale(2, 3);
                    b.stroke();
                </script>
            "#,
    );

    let BlockContent::Canvas(a) = &page.blocks[0].content else {
        panic!("canvas expected");
    };

    let surface = a.canvas.surface();

    assert_eq!(surface.pixel(2, 3), Some(Color::TRANSPARENT));
    assert_eq!(surface.pixel(4, 3), Some(Color::rgba(0, 0, 255, 255)));
    assert_eq!(surface.pixel(3, 6), Some(Color::rgba(255, 0, 0, 255)));
    assert_eq!(surface.pixel(3, 11), Some(Color::rgba(255, 0, 0, 255)));
    assert_eq!(surface.pixel(3, 12), Some(Color::TRANSPARENT));

    let BlockContent::Canvas(b) = &page.blocks[1].content else {
        panic!("canvas expected");
    };

    assert_eq!(
        b.canvas.surface().pixel(3, 0),
        Some(Color::rgba(255, 0, 0, 255))
    );

    assert_eq!(
        b.canvas.surface().pixel(3, 5),
        Some(Color::rgba(255, 0, 0, 255))
    );

    assert_eq!(b.canvas.surface().pixel(3, 6), Some(Color::TRANSPARENT));
}

#[test]
fn canvas_composes_and_resets_transforms() {
    use crate::graphics::raster::Color;

    let page = parse_html(
        "https://example.test/",
        r#"
                <canvas id='a' width='12' height='10'></canvas>

                <script>
                    let ctx = document.getElementById('a').getContext('2d');

                    ctx.translate(3, 1);
                    ctx.save();

                    ctx.transform(1, 0, 1, 1, 2, 0);

                    ctx.fillStyle = 'red';
                    ctx.fillRect(0, 0, 2, 2);

                    ctx.beginPath();
                    ctx.moveTo(0, 3);
                    ctx.lineTo(2, 3);
                    ctx.lineTo(0, 5);
                    ctx.closePath();

                    ctx.resetTransform();

                    ctx.fillStyle = 'blue';
                    ctx.fill();
                    ctx.fillRect(0, 0, 1, 1);

                    ctx.restore();

                    ctx.transform(1, 0, 0, 1, 99, undefined);

                    ctx.fillStyle = 'green';
                    ctx.fillRect(0, 0, 1, 1);
                </script>
            "#,
    );

    let BlockContent::Canvas(a) = &page.blocks[0].content else {
        panic!("canvas expected");
    };

    let surface = a.canvas.surface();

    assert_eq!(surface.pixel(6, 1), Some(Color::rgba(255, 0, 0, 255)));

    assert_eq!(surface.pixel(9, 4), Some(Color::rgba(0, 0, 255, 255)));
    assert_eq!(surface.pixel(0, 0), Some(Color::rgba(0, 0, 255, 255)));
    assert_eq!(surface.pixel(3, 1), Some(Color::rgba(0, 128, 0, 255)));

    assert_eq!(surface.pixel(8, 1), Some(Color::TRANSPARENT));
}

#[test]
fn canvas_set_transform_replaces_and_restores_matrix() {
    use crate::graphics::raster::Color;

    let page = parse_html(
        "https://example.test/",
        r#"
                <canvas id='a' width='12' height='10'></canvas>

                <script>
                    let ctx = document.getElementById('a').getContext('2d');

                    ctx.translate(7, 0);
                    ctx.fillStyle = 'blue';
                    ctx.fillRect(0, 0, 1, 1);

                    ctx.setTransform(2, 0, 0, 3, 1, 2);
                    ctx.fillStyle = 'red';
                    ctx.fillRect(0, 0, 1, 1);

                    ctx.setTransform(1, 0, 0, 1, undefined, 0);
                    ctx.fillStyle = 'green';
                    ctx.fillRect(1, 0, 1, 1);

                    ctx.save();
                    ctx.setTransform();
                    ctx.fillStyle = 'blue';
                    ctx.fillRect(0, 0, 1, 1);

                    ctx.restore();
                    ctx.fillStyle = 'green';
                    ctx.fillRect(2, 0, 1, 1);

                    ctx.setTransform(null);
                    ctx.fillStyle = 'red';
                    ctx.fillRect(0, 7, 1, 1);
                </script>
            "#,
    );

    let BlockContent::Canvas(a) = &page.blocks[0].content else {
        panic!("canvas expected");
    };

    let surface = a.canvas.surface();

    assert_eq!(surface.pixel(7, 0), Some(Color::rgba(0, 0, 255, 255)));
    assert_eq!(surface.pixel(1, 2), Some(Color::rgba(255, 0, 0, 255)));
    assert_eq!(surface.pixel(3, 2), Some(Color::rgba(0, 128, 0, 255)));
    assert_eq!(surface.pixel(0, 0), Some(Color::rgba(0, 0, 255, 255)));
    assert_eq!(surface.pixel(5, 2), Some(Color::rgba(0, 128, 0, 255)));
    assert_eq!(surface.pixel(0, 7), Some(Color::rgba(255, 0, 0, 255)));
}
