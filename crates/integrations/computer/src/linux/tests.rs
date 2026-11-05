use super::{
    act::{chosen, moved, normal},
    atspi::{camel, flags, number, role_name},
    desktop::{arguments, matches, parse},
    label,
    x11::{Keymap, Layout, jpeg, rgb},
};
use crate::Chord;
use atspi_common::{Role, State, StateSet};

#[test]
fn roles_read_as_camel_case_at_spi_names() {
    assert_eq!(role_name(Role::Button), "PushButton");
    assert_eq!(role_name(Role::CheckBox), "CheckBox");
    assert_eq!(role_name(Role::PasswordText), "PasswordText");
    assert_eq!(role_name(Role::PageTabList), "PageTabList");
    assert_eq!(role_name(Role::HTMLContainer), "HtmlContainer");
    assert_eq!(camel("tear off  menu item"), "TearOffMenuItem");
    assert_eq!(camel(""), "");
}

#[test]
fn states_name_what_a_member_can_rely_on() {
    let set = |states: &[State]| {
        let mut set = StateSet::empty();
        for &state in states {
            set.insert(state);
        }
        set
    };
    assert_eq!(
        flags(set(&[State::Enabled, State::Sensitive]), true),
        Vec::<&str>::new()
    );
    assert_eq!(flags(set(&[]), true), ["disabled"]);
    assert_eq!(flags(set(&[]), false), Vec::<&str>::new());
    assert_eq!(
        flags(
            set(&[State::Sensitive, State::Focused, State::Checked]),
            true
        ),
        ["focused", "checked"]
    );
    assert_eq!(
        flags(set(&[State::Enabled, State::Expandable]), true),
        ["collapsed"]
    );
    assert_eq!(
        flags(
            set(&[
                State::Enabled,
                State::Expandable,
                State::Expanded,
                State::Selected
            ]),
            true
        ),
        ["selected", "expanded"]
    );
}

#[test]
fn numbers_read_like_the_mac_engine() {
    assert_eq!(number(42.0), "42");
    assert_eq!(number(-3.0), "-3");
    assert_eq!(number(0.25), "0.25");
}

#[test]
fn actions_match_by_name_whatever_their_spelling() {
    let actions: Vec<String> = ["Activate", "Show Menu", "click"].map(Into::into).into();
    assert_eq!(chosen(&actions, &["click", "press", "activate"]), Some(2));
    assert_eq!(chosen(&actions, &["popup", "menu", "showmenu"]), Some(1));
    assert_eq!(chosen(&actions, &[normal("AXShowMenu").as_str()]), Some(1));
    assert_eq!(chosen(&actions, &[normal("show_menu").as_str()]), Some(1));
    assert_eq!(chosen(&actions, &["toggle"]), None);
    assert_eq!(normal(" AXPress "), "press");
    assert_eq!(normal("Axe"), "axe");
}

#[test]
fn scrolling_moves_a_bar_by_a_share_of_its_range() {
    assert_eq!(moved(0.0, 0.0, 1000.0, 1.0, 1.0), 150.0);
    assert_eq!(moved(500.0, 0.0, 1000.0, -1.0, 2.0), 200.0);
    assert_eq!(moved(900.0, 0.0, 1000.0, 1.0, 5.0), 1000.0);
    assert_eq!(moved(10.0, 0.0, 1000.0, -1.0, 1.0), 0.0);
    assert_eq!(moved(0.0, 0.0, 1000.0, 1.0, 0.0), 15.0);
    assert_eq!(moved(5.0, 5.0, 5.0, 1.0, 1.0), 5.0);
}

#[test]
fn keysyms_find_their_keycode_and_shift_level() {
    let map = Keymap {
        min: 8,
        per: 2,
        syms: vec![
            0x61, 0x41, 0x31, 0x21, 0xff0d, 0, 0x62, 0, 0, 0, 0xffe1, 0, 0, 0,
        ],
    };
    assert_eq!(map.find(0x61), Some((8, false)));
    assert_eq!(map.find(0x41), Some((8, true)));
    assert_eq!(map.find(0x21), Some((9, true)));
    assert_eq!(map.find(0xff0d), Some((10, false)));
    assert_eq!(map.find(0x42), Some((11, true)));
    assert_eq!(map.find(0xffe1), Some((13, false)));
    assert_eq!(map.find(0x0100_20ac), None);
    assert_eq!(map.spare(), Some(14));
    let full = Keymap {
        min: 8,
        per: 1,
        syms: vec![0x61, 0x62],
    };
    assert_eq!(full.spare(), None);
    let wide = Keymap {
        min: 8,
        per: 4,
        syms: vec![0x61, 0x41, 0xe1, 0xc1],
    };
    assert_eq!(wide.find(0xe1), None);
}

#[test]
fn chords_label_their_keys() {
    assert_eq!(
        label(Chord::parse("ctrl+shift+Tab").unwrap()),
        "ctrl+shift+Tab"
    );
    assert_eq!(label(Chord::parse("cmd+a").unwrap()), "super+a");
    assert_eq!(label(Chord::parse("F5").unwrap()), "F5");
    assert_eq!(label(Chord::parse("Page_Down").unwrap()), "PageDown");
}

#[test]
fn x_images_convert_to_rgb() {
    let layout = |bits, msb| Layout {
        bits,
        pad: 32,
        masks: [0xff_0000, 0xff00, 0xff],
        msb,
    };
    let bgrx = [0x30, 0x20, 0x10, 0, 0x03, 0x02, 0x01, 0];
    assert_eq!(
        rgb(&bgrx, 2, 1, &layout(32, false)),
        Some(vec![0x10, 0x20, 0x30, 1, 2, 3])
    );
    let xrgb = [0, 0x10, 0x20, 0x30];
    assert_eq!(
        rgb(&xrgb, 1, 1, &layout(32, true)),
        Some(vec![0x10, 0x20, 0x30])
    );
    let packed = [
        0x30, 0x20, 0x10, 0xff, 0, 0, 0, 0, 0x03, 0x02, 0x01, 0, 0, 0, 0, 0,
    ];
    assert_eq!(
        rgb(
            &packed,
            1,
            2,
            &Layout {
                pad: 64,
                ..layout(24, false)
            }
        ),
        Some(vec![0x10, 0x20, 0x30, 1, 2, 3])
    );
    let rgb565 = [0xff, 0xff];
    let mut short = layout(16, false);
    short.masks = [0xf800, 0x07e0, 0x001f];
    short.pad = 16;
    assert_eq!(rgb(&rgb565, 1, 1, &short), Some(vec![255, 255, 255]));
    assert_eq!(rgb(&bgrx, 2, 2, &layout(32, false)), None);
    assert_eq!(rgb(&bgrx, 1, 1, &layout(8, false)), None);
}

#[test]
fn screenshots_shrink_until_they_fit() {
    let (width, height) = (64u32, 48u32);
    let pixels: Vec<u8> = (0..width * height * 3)
        .map(|n| (n * 7 % 251) as u8)
        .collect();
    let large = jpeg(width, height, &pixels, usize::MAX).unwrap();
    assert!(large.starts_with("/9j/"));
    let small = jpeg(width, height, &pixels, large.len() - 1).unwrap();
    assert!(small.len() < large.len());
    assert_eq!(jpeg(width, height, &pixels, 16), None);
}

#[test]
fn desktop_entries_name_their_program() {
    let entry = parse(
        "# comment\n[Desktop Entry]\nType=Application\nName=Text Editor\nName[de]=Texteditor\nExec=\"/opt/my editor/gedit\" --new-window %U\n[Desktop Action new]\nName=New\nExec=other\n",
    )
    .unwrap();
    assert_eq!(entry.name, "Text Editor");
    assert_eq!(entry.exec, ["/opt/my editor/gedit", "--new-window"]);
    assert!(matches(&entry, "org.gnome.gedit", "text editor"));
    assert!(matches(&entry, "org.gnome.gedit", "org.gnome.Gedit"));
    assert!(matches(&entry, "org.gnome.gedit", "gedit"));
    assert!(!matches(&entry, "org.gnome.gedit", "editor"));
    assert!(parse("[Desktop Entry]\nType=Link\nName=Site\nExec=x\n").is_none());
    assert!(parse("[Desktop Entry]\nType=Application\nName=Gone\nExec=x\nHidden=true\n").is_none());
    assert!(parse("[Desktop Entry]\nType=Application\nName=Nothing\n").is_none());
    assert_eq!(
        arguments("sh -c \"echo \\\"hi\\\"\" 100%% %f --file=%u"),
        ["sh", "-c", "echo \"hi\"", "100%", "--file="]
    );
    assert_eq!(arguments("app \"\" end"), ["app", "", "end"]);
}
