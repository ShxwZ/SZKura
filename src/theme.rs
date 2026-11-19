//! Runtime UI palettes, shared by native egui styling and custom buttons.
//! A small atomic selection avoids rebuilding every widget or editing existing data.
use eframe::egui::Color32;
use std::sync::atomic::{AtomicU8,Ordering};

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
#[repr(u8)]
pub enum ThemeKind { Graphite=0, Sage=1, Purple=2, Orange=3, Vice=4 }

impl ThemeKind {
    pub const ALL:[Self;5]=[Self::Graphite,Self::Sage,Self::Purple,Self::Orange,Self::Vice];
    pub fn id(self)->&'static str {
        match self {
            Self::Graphite=>"graphite",Self::Sage=>"sage",
            Self::Purple=>"purple",Self::Orange=>"orange",
            Self::Vice=>"vice"
        }
    }
    pub fn label(self)->&'static str {
        match self {
            Self::Graphite=>"Ónix",Self::Sage=>"Jade",
            Self::Purple=>"Amatista",Self::Orange=>"Ámbar",
            Self::Vice=>"Vice"
        }
    }
    pub fn from_id(id:&str)->Option<Self>{
        Self::ALL.into_iter().find(|kind|kind.id()==id)
    }
}

#[derive(Clone,Copy)]
pub struct Palette {
    pub bg:Color32, pub panel:Color32, pub card:Color32,
    pub stroke:Color32, pub text:Color32, pub muted:Color32,
    pub accent:Color32, pub primary:Color32,
    pub button_bg:Color32, pub hover_bg:Color32,
    pub nav_bg:Color32, pub nav_selected:Color32,
    pub nav_border:Color32, pub primary_border:Color32,
    pub selection:Color32, pub extreme_bg:Color32,
    pub faint_bg:Color32,
}
fn rgb(r:u8,g:u8,b:u8)->Color32{Color32::from_rgb(r,g,b)}

pub fn swatches(kind:ThemeKind)->Palette {
    match kind {
        ThemeKind::Sage=>Palette {
            bg:rgb(17,23,26), panel:rgb(24,32,35),card:rgb(30,40,43),
            stroke:rgb(51,67,68),text:rgb(232,239,236),muted:rgb(164,180,179),
            accent:rgb(135,194,179),primary:rgb(43,112,104),
            button_bg:rgb(37,47,50),hover_bg:rgb(48,65,65),
            nav_bg:rgb(26,35,38),nav_selected:rgb(38,91,85),
            nav_border:rgb(72,148,135),primary_border:rgb(63,141,128),
            selection:rgb(46,110,101),extreme_bg:rgb(20,28,31),
            faint_bg:rgb(31,42,44),
        },
        ThemeKind::Graphite=>Palette {
            bg:rgb(15,16,18),panel:rgb(24,25,28),card:rgb(34,35,39),
            stroke:rgb(61,62,68),text:rgb(237,238,240),muted:rgb(165,168,175),
            accent:rgb(204,208,216),primary:rgb(81,86,96),
            button_bg:rgb(41,43,47),hover_bg:rgb(58,61,67),
            nav_bg:rgb(29,31,34),nav_selected:rgb(78,82,91),
            nav_border:rgb(131,137,149),primary_border:rgb(143,149,158),
            selection:rgb(91,96,108),extreme_bg:rgb(20,21,24),
            faint_bg:rgb(37,39,44),
        },
        ThemeKind::Purple=>Palette {
            bg:rgb(21,17,29),panel:rgb(31,25,41),card:rgb(42,34,53),
            stroke:rgb(73,58,89),text:rgb(242,236,247),muted:rgb(185,171,196),
            accent:rgb(211,167,244),primary:rgb(112,69,154),
            button_bg:rgb(49,39,61),hover_bg:rgb(67,50,88),
            nav_bg:rgb(35,28,46),nav_selected:rgb(89,57,122),
            nav_border:rgb(155,107,203),primary_border:rgb(159,111,206),
            selection:rgb(113,73,154),extreme_bg:rgb(25,20,34),
            faint_bg:rgb(43,33,56),
        },
        ThemeKind::Vice=>Palette {
            // Midnight violet panels and opaque, distinct cards. The sunset
            // belongs to the background, not to interactive surfaces.
            bg:rgb(23,11,41),panel:rgb(31,13,50),card:rgb(52,24,67),
            stroke:rgb(105,57,100),text:rgb(248,238,250),muted:rgb(197,171,195),
            accent:rgb(243,126,181),primary:rgb(159,48,105),
            button_bg:rgb(49,24,64),hover_bg:rgb(78,35,79),
            nav_bg:rgb(34,14,58),nav_selected:rgb(88,34,84),
            nav_border:rgb(229,105,162),primary_border:rgb(234,113,164),
            selection:rgb(126,47,102),extreme_bg:rgb(24,11,42),
            faint_bg:rgb(43,24,53),
        },
        ThemeKind::Orange=>Palette {
            bg:rgb(25,20,17),panel:rgb(35,27,23),card:rgb(46,36,29),
            stroke:rgb(87,67,53),text:rgb(245,237,227),muted:rgb(195,177,161),
            accent:rgb(247,184,121),primary:rgb(151,83,42),
            button_bg:rgb(55,41,32),hover_bg:rgb(77,55,41),
            nav_bg:rgb(39,29,24),nav_selected:rgb(111,66,37),
            nav_border:rgb(197,119,61),primary_border:rgb(210,128,65),
            selection:rgb(145,84,44),extreme_bg:rgb(30,23,20),
            faint_bg:rgb(49,36,29),
        }
    }
}

/// Subdued sunset (top-left, top-right, bottom-right, bottom-left).
/// It should read as atmosphere rather than a bright orange surface.
fn vice_gradient_colors()->[Color32;4]{
    [rgb(26,12,45),rgb(38,15,51),rgb(72,37,38),rgb(40,16,45)]
}

/// Root panels must not paint opaque rectangles over the full-window
/// background. All existing palettes keep their original solid fills.
fn content_fill_for(kind:ThemeKind)->Color32{
    if kind==ThemeKind::Vice{Color32::TRANSPARENT}else{swatches(kind).bg}
}
pub fn content_panel_fill()->Color32{content_fill_for(selected())}

/// Slightly tinted chrome preserves navigation contrast, but the same global
/// Vice sunset remains visible underneath the header, sidebar and status bar.
fn chrome_fill_for(kind:ThemeKind)->Color32{
    if kind==ThemeKind::Vice{
        Color32::from_rgba_unmultiplied(23,11,42,166)
    }else{swatches(kind).panel}
}
pub fn chrome_panel_fill()->Color32{chrome_fill_for(selected())}

/// Paint ONE static gradient in window coordinates, before any egui panels.
/// Previously each CentralPanel drew its own gradient in local coordinates,
/// causing a hard rectangular edge where it met the header or sidebar.
pub fn paint_vice_background(ctx:&eframe::egui::Context){
    if selected()!=ThemeKind::Vice{return;}
    let rect=ctx.content_rect();
    if rect.width()<=0.0 || rect.height()<=0.0{return;}
    let [top_left,top_right,bottom_right,bottom_left]=vice_gradient_colors();
    let mut mesh=eframe::egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(),top_left);
    mesh.colored_vertex(rect.right_top(),top_right);
    mesh.colored_vertex(rect.right_bottom(),bottom_right);
    mesh.colored_vertex(rect.left_bottom(),bottom_left);
    mesh.add_triangle(0,1,2);
    mesh.add_triangle(0,2,3);
    ctx.layer_painter(eframe::egui::LayerId::background())
        .add(eframe::egui::Shape::mesh(mesh));
}

/// Apply the active palette to ALL egui widget surfaces.
/// Buttons and ComboBoxes use `weak_bg_fill` (not only `bg_fill`);
/// expanded ComboBoxes use the separate `open` widget state.
pub fn apply_widget_visuals(visuals:&mut eframe::egui::Visuals,p:Palette){
    let w=&mut visuals.widgets;
    let stroke=|color|eframe::egui::Stroke::new(1.0,color);
    // egui dims disabled widgets separately; retain a muted opaque surface.
    w.noninteractive.bg_fill=p.panel;
    w.noninteractive.weak_bg_fill=p.panel;
    w.noninteractive.bg_stroke=stroke(p.stroke);
    w.noninteractive.fg_stroke.color=p.text;
    w.inactive.bg_fill=p.button_bg;
    w.inactive.weak_bg_fill=p.button_bg;
    w.inactive.bg_stroke=stroke(p.stroke);
    w.inactive.fg_stroke.color=p.text;
    w.hovered.bg_fill=p.hover_bg;
    w.hovered.weak_bg_fill=p.hover_bg;
    w.hovered.bg_stroke=stroke(p.accent);
    w.hovered.fg_stroke.color=p.text;
    w.active.bg_fill=p.primary;
    w.active.weak_bg_fill=p.primary;
    w.active.bg_stroke=stroke(p.primary_border);
    w.active.fg_stroke.color=p.text;
    w.open.bg_fill=p.hover_bg;
    w.open.weak_bg_fill=p.hover_bg;
    w.open.bg_stroke=stroke(p.accent);
    w.open.fg_stroke.color=p.text;
}

static ACTIVE:AtomicU8=AtomicU8::new(ThemeKind::Sage as u8);
pub fn selected()->ThemeKind{
    match ACTIVE.load(Ordering::Relaxed){
        0=>ThemeKind::Graphite,2=>ThemeKind::Purple,
        3=>ThemeKind::Orange,4=>ThemeKind::Vice,
        _=>ThemeKind::Sage,
    }
}
pub fn select(kind:ThemeKind){ACTIVE.store(kind as u8,Ordering::Relaxed);}
pub fn colors()->Palette{swatches(selected())}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn palette_ids_round_trip_and_are_distinct(){
        let mut ids=std::collections::HashSet::new();
        for kind in ThemeKind::ALL {
            assert_eq!(ThemeKind::from_id(kind.id()),Some(kind));
            assert!(ids.insert(kind.id()));
            let p=swatches(kind);
            assert_ne!(p.bg,p.card);
            assert_ne!(p.text,p.bg);
            assert_ne!(p.primary,p.panel);
        }
        // Internal persisted IDs are deliberately unchanged; only display labels
        // change so existing SQLite settings and restored backups keep working.
        assert_eq!(ThemeKind::Graphite.id(),"graphite");
        assert_eq!(ThemeKind::Sage.id(),"sage");
        assert_eq!(ThemeKind::Purple.id(),"purple");
        assert_eq!(ThemeKind::Orange.id(),"orange");
        assert_eq!(ThemeKind::Vice.id(),"vice");
        let labels=ThemeKind::ALL.map(ThemeKind::label);
        assert_eq!(labels,["Ónix","Jade","Amatista","Ámbar","Vice"]);
        assert_eq!(labels.iter().copied().collect::<std::collections::HashSet<_>>().len(),5);
        let sunset=vice_gradient_colors();
        assert!(sunset[0].b()>sunset[0].r(), "Vice starts purple");
        assert!(sunset[2].r()>sunset[2].b(), "Vice ends warm");
        assert!(swatches(ThemeKind::Vice).accent.r()>swatches(ThemeKind::Vice).accent.b());
        // The sunset is secondary; cards stay more distinct from the panel.
        assert_eq!(content_fill_for(ThemeKind::Vice),Color32::TRANSPARENT);
        assert!(chrome_fill_for(ThemeKind::Vice).a()>0);
        assert!(chrome_fill_for(ThemeKind::Vice).a()<255);
        for kind in [ThemeKind::Graphite,ThemeKind::Sage,
            ThemeKind::Purple,ThemeKind::Orange] {
            assert_eq!(content_fill_for(kind),swatches(kind).bg);
            assert_eq!(chrome_fill_for(kind),swatches(kind).panel);
        }
        let vice=swatches(ThemeKind::Vice);
        assert!(vice.card.r()>vice.panel.r());
        assert!(vice.card.g()>vice.panel.g());
        assert!(vice.card.b()>vice.panel.b());
        assert!(vice.accent.r()>vice.card.r());
        // Lower-right warmth is deliberately restrained compared with neon accents.
        assert!(sunset[2].r()<vice.accent.r()/2);
        assert_eq!(ThemeKind::from_id("unknown"),None);
        assert_eq!(ThemeKind::from_id(""),None);
    }
    #[test]
    fn native_buttons_and_dropdowns_follow_every_palette(){
        for kind in ThemeKind::ALL {
            let p=swatches(kind);
            let mut visuals=eframe::egui::Visuals::dark();
            apply_widget_visuals(&mut visuals,p);
            let w=&visuals.widgets;
            assert_eq!(w.noninteractive.bg_fill,p.panel);
            assert_eq!(w.inactive.bg_fill,p.button_bg);
            assert_eq!(w.inactive.weak_bg_fill,p.button_bg);
            assert_eq!(w.hovered.weak_bg_fill,p.hover_bg);
            assert_eq!(w.active.weak_bg_fill,p.primary);
            assert_eq!(w.open.bg_fill,p.hover_bg);
            assert_eq!(w.open.weak_bg_fill,p.hover_bg);
            assert_eq!(w.inactive.bg_stroke.color,p.stroke);
            assert_eq!(w.hovered.bg_stroke.color,p.accent);
            assert_eq!(w.active.bg_stroke.color,p.primary_border);
            assert_eq!(w.open.bg_stroke.color,p.accent);
            assert_eq!(w.open.fg_stroke.color,p.text);
        }
    }
}