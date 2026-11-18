//! Portable UI resources. Text icons are deliberately avoided so system glyph coverage
//! never controls the appearance of buttons and navigation.
use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, Painter, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use std::path::Path;
use crate::theme;
use crate::i18n::tr;

#[derive(Clone, Copy)]
pub enum Icon {
    Back, Plus, Folder, Link, Grid, Archive, Image, Refresh, Search, Trash, Filter, File, Star,
}

pub fn install_cjk_font(ctx:&egui::Context)->bool {
    // Egui's built-in latin/emoji fonts lack Japanese glyphs such as 【21アバター対応】.
    // Use ONE OS font as a fallback: never package fonts into the executable.
    #[cfg(windows)]
    let candidates:&[&str]=&[
        r"C:\Windows\Fonts\YuGothR.ttc",
        r"C:\Windows\Fonts\YuGothM.ttc",
        r"C:\Windows\Fonts\meiryo.ttc",
        r"C:\Windows\Fonts\msgothic.ttc",
        r"C:\Windows\Fonts\msmincho.ttc",
    ];
    #[cfg(target_os="linux")]
    let candidates:&[&str]=&[
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansJP-Regular.ttf",
        "/usr/share/fonts/opentype/noto/NotoSansJP-Regular.otf",
        "/usr/share/fonts/truetype/fonts-japanese-gothic.ttf",
        "/usr/share/fonts/truetype/takao-gothic/TakaoPGothic.ttf",
        "/usr/share/fonts/truetype/ipafont-gothic/ipag.ttf",
    ];
    #[cfg(not(any(windows,target_os="linux")))]
    let candidates:&[&str]=&[];
    let Some(bytes)=candidates.iter().find_map(|path|{
        if Path::new(path).is_file(){std::fs::read(path).ok()}else{None}
    })else{return false;};
    let mut fonts=FontDefinitions::default();
    fonts.font_data.insert("SZ_CJK".to_owned(),FontData::from_owned(bytes).into());
    fonts.families.entry(FontFamily::Proportional).or_default().push("SZ_CJK".into());
    fonts.families.entry(FontFamily::Monospace).or_default().push("SZ_CJK".into());
    ctx.set_fonts(fonts);
    true
}

pub fn draw_icon(p:&Painter, r:Rect, symbol:Icon, color:Color32){
    let s=r.width().min(r.height());
    let at=|x:f32,y:f32|Pos2::new(r.left()+s*x,r.top()+s*y);
    let line=Stroke::new((s*0.08).max(1.4),color);
    match symbol {
        Icon::Back=>{
            p.line_segment([at(0.70,0.50),at(0.26,0.50)],line);
            p.line_segment([at(0.26,0.50),at(0.47,0.29)],line);
            p.line_segment([at(0.26,0.50),at(0.47,0.71)],line);
        }
        Icon::Plus=>{
            p.line_segment([at(0.50,0.20),at(0.50,0.80)],line);
            p.line_segment([at(0.20,0.50),at(0.80,0.50)],line);
        }
        Icon::Folder=>{
            p.line_segment([at(0.14,0.32),at(0.40,0.32)],line);
            p.line_segment([at(0.40,0.32),at(0.49,0.42)],line);
            p.line_segment([at(0.49,0.42),at(0.86,0.42)],line);
            p.rect_stroke(Rect::from_min_max(at(0.14,0.42),at(0.86,0.80)),
                2.0,line,egui::StrokeKind::Inside);
        }
        Icon::Link=>{
            p.line_segment([at(0.37,0.61),at(0.63,0.39)],line);
            p.line_segment([at(0.30,0.43),at(0.22,0.51)],line);
            p.line_segment([at(0.22,0.51),at(0.22,0.71)],line);
            p.line_segment([at(0.22,0.71),at(0.42,0.77)],line);
            p.line_segment([at(0.58,0.23),at(0.78,0.29)],line);
            p.line_segment([at(0.78,0.29),at(0.78,0.49)],line);
            p.line_segment([at(0.78,0.49),at(0.70,0.57)],line);
        }
        Icon::Grid=>{
            for x in [0.18,0.53]{for y in [0.18,0.53]{
                p.rect_stroke(Rect::from_min_max(at(x,y),at(x+0.27,y+0.27)),2.0,line,egui::StrokeKind::Inside);
            }}
        }
        Icon::Archive=>{
            p.rect_stroke(Rect::from_min_max(at(0.17,0.30),at(0.83,0.78)),2.0,line,egui::StrokeKind::Inside);
            p.line_segment([at(0.15,0.24),at(0.85,0.24)],line);
            p.line_segment([at(0.40,0.48),at(0.60,0.48)],line);
        }
        Icon::Image=>{
            p.rect_stroke(Rect::from_min_max(at(0.16,0.19),at(0.84,0.81)),2.0,line,egui::StrokeKind::Inside);
            p.circle_stroke(at(0.63,0.36),s*0.07,line);
            p.line_segment([at(0.20,0.72),at(0.42,0.48)],line);
            p.line_segment([at(0.42,0.48),at(0.58,0.64)],line);
            p.line_segment([at(0.58,0.64),at(0.73,0.50)],line);
        }
        Icon::Refresh=>{
            p.circle_stroke(at(0.50,0.50),s*0.27,line);
            p.line_segment([at(0.70,0.18),at(0.77,0.39)],line);
            p.line_segment([at(0.77,0.39),at(0.55,0.37)],line);
        }
        Icon::Search=>{
            p.circle_stroke(at(0.43,0.43),s*0.23,line);
            p.line_segment([at(0.60,0.60),at(0.82,0.82)],line);
        }
        Icon::Trash=>{
            p.rect_stroke(Rect::from_min_max(at(0.29,0.32),at(0.71,0.79)),2.0,line,egui::StrokeKind::Inside);
            p.line_segment([at(0.20,0.27),at(0.80,0.27)],line);
            p.line_segment([at(0.39,0.19),at(0.61,0.19)],line);
        }
        Icon::Filter=>{
            p.line_segment([at(0.17,0.26),at(0.83,0.26)],line);
            p.line_segment([at(0.17,0.26),at(0.43,0.55)],line);
            p.line_segment([at(0.83,0.26),at(0.57,0.55)],line);
            p.line_segment([at(0.43,0.55),at(0.43,0.78)],line);
            p.line_segment([at(0.43,0.78),at(0.57,0.66)],line);
            p.line_segment([at(0.57,0.66),at(0.57,0.55)],line);
        }
        Icon::Star=>{
            let points=(0..10).map(|i|{
                let angle=std::f32::consts::PI*(i as f32)/5.0-std::f32::consts::FRAC_PI_2;
                let radius=if i%2==0 {0.39} else {0.18};
                Pos2::new(r.center().x+radius*s*angle.cos(),
                    r.center().y+radius*s*angle.sin())
            }).collect::<Vec<_>>();
            p.add(egui::Shape::closed_line(points,line));
        }
        Icon::File=>{
            p.line_segment([at(0.25,0.18),at(0.61,0.18)],line);
            p.line_segment([at(0.61,0.18),at(0.77,0.35)],line);
            p.line_segment([at(0.77,0.35),at(0.77,0.82)],line);
            p.line_segment([at(0.77,0.82),at(0.25,0.82)],line);
            p.line_segment([at(0.25,0.82),at(0.25,0.18)],line);
            p.line_segment([at(0.61,0.18),at(0.61,0.35)],line);
            p.line_segment([at(0.61,0.35),at(0.77,0.35)],line);
        }
    }
}

/// Shared geometry for LOCAL and REMOTE catalog covers: letterbox the sharp
/// image over the same rounded blurred backdrop. Never stretch or crop assets.
pub fn fitted_cover_rect(frame:Rect,natural:Vec2)->Rect{
    if natural.x<=0.0 || natural.y<=0.0{
        return Rect::from_center_size(frame.center(),Vec2::ZERO);
    }
    let factor=(frame.width()/natural.x).min(frame.height()/natural.y)
        .min(1.0).max(0.0);
    Rect::from_center_size(frame.center(),natural*factor)
}

/// Exactly one cover painter in SZKura. Texture loading and decoding happen
/// in their respective background workers; this only draws ready textures.
pub fn paint_cover(ui:&mut Ui,foreground:Option<&egui::TextureHandle>,
    backdrop:Option<&egui::TextureHandle>,width:f32,height:f32){
    let (rect,_)=ui.allocate_exact_size(egui::vec2(width,height),Sense::hover());
    ui.painter().rect_filled(rect,10.0,theme::colors().faint_bg);
    if let Some(tex)=foreground {
        if let Some(bg)=backdrop {
            egui::Image::new((bg.id(),bg.size_vec2()))
                .corner_radius(10)
                .tint(Color32::from_rgb(210,210,210))
                .paint_at(ui,rect);
            ui.painter().rect_filled(rect,10.0,Color32::from_black_alpha(45));
        }
        let bounds=fitted_cover_rect(rect,tex.size_vec2());
        egui::Image::new((tex.id(),tex.size_vec2()))
            .corner_radius(4).paint_at(ui,bounds);
    }else{
        ui.painter().text(rect.center(),egui::Align2::CENTER_CENTER,
            tr("Sin portada"),egui::FontId::proportional(13.0),theme::colors().muted);
    }
}

pub fn icon(ui:&mut Ui,symbol:Icon,color:Color32,size:f32){
    let (rect,_) = ui.allocate_exact_size(Vec2::splat(size),Sense::hover());
    draw_icon(ui.painter(),rect,symbol,color);
}

// Unified controls: 36 pt height, no literal blank-space padding to fake icon width.
const ACTION_HEIGHT: f32 = 36.0;
const ACTION_ICON_SIZE: f32 = 16.0;
const ACTION_GAP: f32 = 8.0;
const ACTION_PAD_X: f32 = 12.0;
const ACTION_TEXT_SIZE: f32 = 13.5;

/// Center the icon-and-label *as one group*. Its geometry does not depend
/// on font-dependent spaces, Windows display scaling, or label length.
fn icon_text_layout(button:Rect,text_size:Vec2)->(Rect,Pos2){
    let total_width=ACTION_ICON_SIZE+ACTION_GAP+text_size.x;
    let left=button.center().x-total_width*0.5;
    let icon_rect=Rect::from_min_size(
        Pos2::new(left,button.center().y-ACTION_ICON_SIZE*0.5),
        Vec2::splat(ACTION_ICON_SIZE));
    let text_pos=Pos2::new(icon_rect.right()+ACTION_GAP,
        button.center().y-text_size.y*0.5);
    (icon_rect,text_pos)
}

/// A real, accessible egui Button retains its original text label, but
/// renders content through measured centered geometry rather than blank spaces.
pub fn action(ui:&mut Ui,symbol:Icon,label:&str,primary:bool)->bool{
    let label=tr(label);
    let p=theme::colors();
    let text_color=if ui.is_enabled(){p.text}else{p.muted};
    let galley=ui.painter().layout_no_wrap(
        label.to_owned(),egui::FontId::proportional(ACTION_TEXT_SIZE),text_color);
    let button_width=ACTION_PAD_X*2.0+ACTION_ICON_SIZE+ACTION_GAP+galley.size().x;
    // Keep the native label for screen readers; custom rendering centers
    // the icon and text together. Transparent text never adds visual spacing.
    let button=egui::Button::new(
        egui::RichText::new(label).size(ACTION_TEXT_SIZE)
            .color(Color32::TRANSPARENT))
        .min_size(Vec2::new(button_width,ACTION_HEIGHT))
        .corner_radius(8.0)
        .fill(if primary{p.primary}else{p.button_bg})
        .stroke(Stroke::new(1.0,if primary{p.primary_border}else{p.stroke}));
    let response=ui.add(button);
    let (icon_rect,text_pos)=icon_text_layout(response.rect,galley.size());
    let painter=ui.painter();
    draw_icon(painter,icon_rect,symbol,text_color);
    painter.galley(text_pos,galley,text_color);
    response.clicked()
}

/// Icon-only action: no dummy space glyph. Its 36x36 body and icon share a center.
pub fn icon_action(ui:&mut Ui,symbol:Icon,tooltip:&str,danger:bool)->bool{
    let tooltip=tr(tooltip);
    let p=theme::colors();
    let response=ui.add(egui::Button::new("")
        .min_size(Vec2::splat(ACTION_HEIGHT))
        .corner_radius(8.0)
        .fill(p.button_bg)
        .stroke(Stroke::new(1.0,p.stroke)))
        .on_hover_text(tooltip);
    let side=17.0;
    let rect=Rect::from_center_size(response.rect.center(),Vec2::splat(side));
    let tint=if !ui.is_enabled(){p.muted}
        else if danger{Color32::from_rgb(220,147,138)}
        else{p.text};
    draw_icon(ui.painter(),rect,symbol,tint);
    response.clicked()
}

/// Selected favorites are visibly highlighted, not merely marked in SQLite.
pub fn favorite_action(ui:&mut Ui,selected:bool)->bool{
    let p=theme::colors();
    let tooltip=tr(if selected{"Quitar de favoritos"}else{"Añadir a favoritos"});
    let response=ui.add(egui::Button::new("")
        .min_size(Vec2::splat(ACTION_HEIGHT))
        .corner_radius(8.0)
        .fill(if selected{p.nav_selected}else{p.button_bg})
        .stroke(Stroke::new(1.0,if selected{p.nav_border}else{p.stroke})))
        .on_hover_text(tooltip);
    let rect=Rect::from_center_size(response.rect.center(),Vec2::splat(18.0));
    draw_icon(ui.painter(),rect,Icon::Star,
        if selected{Color32::from_rgb(249,215,123)}else{p.text});
    response.clicked()
}

/// Categories and file filters use the same height, spacing and selection style.
pub fn nav_choice(ui:&mut Ui,label:&str,selected:bool)->bool{
    let label=tr(label);
    let p=theme::colors();
    let button=egui::Button::new(egui::RichText::new(label)
        .size(13.0).color(if selected{p.text}else{p.muted}))
        .min_size(Vec2::new(184.0,ACTION_HEIGHT))
        .corner_radius(8.0)
        .fill(if selected{p.nav_selected}else{p.nav_bg})
        .stroke(Stroke::new(1.0,if selected{p.nav_border}else{p.nav_bg}));
    ui.add(button).clicked()
}

/// Content-visibility choice uses the same geometry as categories and favorites.
/// Draw the circle separately from text: font metrics and CJK fallbacks cannot move it.
fn sensitive_choice_positions(rect:Rect,text_height:f32)->(Pos2,Pos2){
    (Pos2::new(rect.left()+14.0,rect.center().y),
     Pos2::new(rect.left()+30.0,rect.center().y-text_height*0.5))
}
pub fn nav_sensitive_choice(ui:&mut Ui,label:&str,selected:bool)->bool{
    let label=tr(label);
    let p=theme::colors();
    let text_color=if selected{Color32::from_rgb(246,227,250)}else{p.muted};
    let galley=ui.painter().layout_no_wrap(
        label.to_owned(),egui::FontId::proportional(13.0),text_color);
    // Preserve the real button label for accessibility; paint its visible
    // content at fixed coordinates, independent of any bullet glyph baseline.
    let button=egui::Button::new(egui::RichText::new(label)
            .size(13.0).color(Color32::TRANSPARENT))
        .min_size(Vec2::new(184.0,ACTION_HEIGHT))
        .corner_radius(8.0)
        .fill(if selected{Color32::from_rgb(89,58,100)}else{p.nav_bg})
        .stroke(Stroke::new(1.0,if selected {
            Color32::from_rgb(154,113,177)
        }else{p.nav_bg}));
    let response=ui.add(button).on_hover_text(
        tr("Alternar visibilidad de las fichas marcadas como NSFW. Se ocultan por defecto.")
    );
    let (circle,text_pos)=sensitive_choice_positions(response.rect,galley.size().y);
    if selected {
        ui.painter().circle_filled(circle,5.5,Color32::from_rgb(246,227,250));
    }else{
        ui.painter().circle_stroke(circle,5.5,Stroke::new(1.5,p.muted));
    }
    ui.painter().galley(text_pos,galley,text_color);
    response.clicked()
}


/* Shared confirmation-window UI. Use this instead of styling each egui::Window
 * separately. The only differences between confirmations are their message and
 * semantic action tone. See docs/UI_DESIGN_GUIDE.md. */
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub enum ConfirmChoice { Pending, Cancel, Accept }

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub enum ConfirmTone { Primary, Destructive }

pub fn dialog_subject(ui:&mut Ui,subject:&str){
    let p=theme::colors();
    ui.add(egui::Label::new(
        egui::RichText::new(subject).strong().color(p.accent).size(14.0)).wrap());
}

pub fn dialog_text(ui:&mut Ui,message:&str){
    let p=theme::colors();
    ui.add(egui::Label::new(
        egui::RichText::new(crate::i18n::tr_message(message).to_string()).color(p.muted).size(13.0)).wrap());
}

/// One centered, non-resizable visual style with no duplicate native title bar.
/// Cancel remains possible via both the button and Escape.
pub fn confirmation_dialog(
    ctx:&egui::Context, id:&'static str, heading:&str,
    action_label:&str, tone:ConfirmTone, enabled:bool,
    body:impl FnOnce(&mut Ui),
)->ConfirmChoice {
    let p=theme::colors();
    let mut choice=ConfirmChoice::Pending;
    let width=(ctx.content_rect().width()-80.0).clamp(300.0,440.0);
    // Modal sizes itself to its content and blocks background input.
    // Window retained a much taller geometry when nested layouts expanded.
    let modal=egui::Modal::new(egui::Id::new(id))
        .frame(egui::Frame::new()
            .fill(p.panel)
            .stroke(Stroke::new(1.0,p.stroke))
            .corner_radius(12.0)
            .inner_margin(egui::Margin::symmetric(20,18)))
        .show(ctx,|ui|{
            ui.set_width(width);
            ui.label(egui::RichText::new(tr(heading))
                .strong().size(19.0).color(p.text));
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(12.0);
            body(ui);
            ui.add_space(18.0);
            ui.separator();
            ui.add_space(12.0);
            // horizontal isolates the button row's 36pt height from the
            // available height of its enclosing vertical layout.
            ui.horizontal(|ui|{
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                let action_color=if tone==ConfirmTone::Destructive{
                    Color32::from_rgb(132,65,63)
                }else{p.primary};
                let action=egui::Button::new(
                    egui::RichText::new(tr(action_label)).size(13.0).color(p.text))
                    .min_size(Vec2::new(0.0,ACTION_HEIGHT))
                    .corner_radius(8.0)
                    .fill(action_color);
                if ui.add_enabled(enabled,action).clicked(){
                    choice=ConfirmChoice::Accept;
                }
                ui.add_space(8.0);
                let cancel=egui::Button::new(
                    egui::RichText::new(tr("Cancelar")).size(13.0).color(p.text))
                    .min_size(Vec2::new(0.0,ACTION_HEIGHT))
                    .corner_radius(8.0)
                    .fill(p.button_bg)
                    .stroke(Stroke::new(1.0,p.stroke));
                if ui.add(cancel).clicked(){choice=ConfirmChoice::Cancel;}
                });
            });
        });
    if modal.should_close(){ConfirmChoice::Cancel}else{choice}
}

#[cfg(test)]
mod alignment_tests {
    use super::*;

    #[test]
    fn confirmation_modal_tracks_content_instead_of_filling_window() {
        let ctx=egui::Context::default();
        let viewport=egui::Rect::from_min_size(
            egui::Pos2::ZERO,egui::vec2(1024.0,768.0));
        for _ in 0..3 {
            let _=ctx.run(egui::RawInput {
                screen_rect:Some(viewport),
                ..Default::default()
            },|ctx|{
                let _=confirmation_dialog(ctx,"modal-layout-test",
                    "Eliminar del catálogo","Eliminar ficha",
                    ConfirmTone::Destructive,true,|ui|{
                        dialog_subject(ui,"Ficha de ejemplo");
                        ui.add_space(8.0);
                        dialog_text(ui,"Solo se quita del catálogo y sus archivos se conservan.");
                    });
            });
        }
        let rect=ctx.memory(|m|m.area_rect(egui::Id::new("modal-layout-test")))
            .expect("El modal debe ocupar un área propia");
        assert!(rect.height()<300.0,
            "Un diálogo breve no debería medir {} puntos de alto",rect.height());
        assert!(rect.width()>=300.0 && rect.width()<=540.0);
    }

    #[test]
    fn sensitive_circle_and_label_are_vertically_centered() {
        for top in [0.0,19.5,114.0] {
            let rect=Rect::from_min_size(Pos2::new(8.0,top),Vec2::new(184.0,ACTION_HEIGHT));
            for text_height in [12.0,15.0,19.0] {
                let (circle,text)=sensitive_choice_positions(rect,text_height);
                assert_eq!(circle.y,rect.center().y);
                assert!((text.y+text_height*0.5-circle.y).abs()<0.01);
                assert!(text.x>circle.x);
            }
        }
    }

    #[test]
    fn centered_icon_and_text_share_the_button_center() {
        for text_width in [20.0,73.0,195.5] {
            let button=Rect::from_min_size(Pos2::new(17.0,29.0),Vec2::new(260.0,36.0));
            let text_size=Vec2::new(text_width,16.0);
            let (icon_rect,text_pos)=icon_text_layout(button,text_size);
            let whole_left=icon_rect.left();
            let whole_right=text_pos.x+text_width;
            let whole_center=(whole_left+whole_right)*0.5;
            assert!((whole_center-button.center().x).abs()<0.01);
            assert!((icon_rect.center().y-button.center().y).abs()<0.01);
            assert!((text_pos.y+text_size.y*0.5-button.center().y).abs()<0.01);
            assert!((text_pos.x-icon_rect.right()-ACTION_GAP).abs()<0.01);
        }
    }
}
