use super::{
    act::{chosen, moved, normal},
    atspi::{camel, flags, number, role_name},
    desktop::{Entry, Listed, arguments, interpreter, matches, names, parse},
    identity::{Process, by_class, by_exec},
    label,
    x11::{Keymap, Layout, jpeg, rgb},
};
use crate::Chord;
use atspi_common::{Role, State, StateSet};
use std::fs;

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
    let chrome = parse(
        "[Desktop Entry]\nType=Application\nName=Chrome\nExec=chrome\nStartupWMClass=Google-chrome\n",
    )
    .unwrap();
    assert_eq!(chrome.wm_class.as_deref(), Some("Google-chrome"));
    assert_eq!(entry.wm_class, None);
    assert_eq!(
        arguments("sh -c \"echo \\\"hi\\\"\" 100%% %f --file=%u"),
        ["sh", "-c", "echo \"hi\"", "100%", "--file="]
    );
    assert_eq!(arguments("app \"\" end"), ["app", "", "end"]);
}

fn listed(id: &str, exec: &str, wm_class: Option<&str>, program: &str) -> Listed {
    Listed {
        id: id.into(),
        path: format!("/usr/share/applications/{id}.desktop"),
        entry: Entry {
            name: id.into(),
            exec: arguments(exec),
            wm_class: wm_class.map(Into::into),
        },
        program: Some(program.into()),
    }
}

fn process(exe: &str, args: &[&str], environ: &str) -> Process {
    Process {
        exe: exe.into(),
        args: args.iter().map(|&arg| arg.to_owned()).collect(),
        cwd: None,
        environ: environ.replace(' ', "\0").into_bytes(),
    }
}

#[test]
fn a_launched_app_names_its_desktop_entry_but_its_children_do_not() {
    let launched = process(
        "/usr/bin/python3.12",
        &["python3", "/opt/notes/notes.py"],
        "HOME=/home/a GIO_LAUNCHED_DESKTOP_FILE=/usr/share/applications/org.example.Notes.desktop GIO_LAUNCHED_DESKTOP_FILE_PID=42",
    );
    assert_eq!(
        launched.launched_from(42).as_deref(),
        Some("org.example.Notes")
    );
    assert_eq!(launched.launched_from(43), None);
    let hinted = process(
        "/usr/lib/firefox/firefox",
        &["firefox"],
        "BAMF_DESKTOP_FILE_HINT=/usr/share/applications/firefox.desktop",
    );
    assert_eq!(hinted.launched_from(7).as_deref(), Some("firefox"));
    assert_eq!(
        process("/usr/bin/gedit", &["gedit"], "PATH=/bin").launched_from(7),
        None
    );
}

#[test]
fn interpreters_are_known_by_their_script() {
    for program in [
        "python3.12",
        "python3",
        "perl5.36.0",
        "node",
        "bash",
        "java",
        "gjs",
    ] {
        assert!(interpreter(program), "{program}");
    }
    for program in ["gedit", "python-gui", "firefox", ""] {
        assert!(!interpreter(program), "{program}");
    }
    let notes = process(
        "/usr/bin/python3.12",
        &["python3", "-u", "notes.py", "-v"],
        "",
    );
    assert_eq!(notes.script(), Some((2, "notes.py")));
    assert_eq!(
        process("/usr/bin/python3.12", &["python3"], "").script(),
        None
    );
    assert_eq!(
        process("/usr/bin/gedit", &["gedit", "a.txt"], "").script(),
        None
    );
    assert_eq!(
        names([
            Some("org.example.Notes".into()),
            None,
            Some("notes.py".into()),
            Some("".into()),
            Some("ORG.example.notes".into()),
            Some("python3.12".into()),
        ]),
        ["org.example.Notes", "notes.py", "python3.12"]
    );
}

#[test]
fn windows_find_their_entry_by_application_id_or_class() {
    let entries = [
        listed(
            "org.gnome.TextEditor",
            "gnome-text-editor %U",
            None,
            "/usr/bin/gnome-text-editor",
        ),
        listed(
            "google-chrome",
            "/usr/bin/google-chrome-stable %U",
            Some("Google-chrome-beta"),
            "/opt/google/chrome/google-chrome",
        ),
    ];
    let classes = |names: &[&str]| {
        names
            .iter()
            .map(|&name| name.to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        by_class(
            &entries,
            &classes(&["org.gnome.TextEditor", "gnome-text-editor"])
        )
        .as_deref(),
        Some("org.gnome.TextEditor")
    );
    assert_eq!(
        by_class(
            &entries,
            &classes(&["google-chrome-beta", "Google-chrome-beta"])
        )
        .as_deref(),
        Some("google-chrome")
    );
    assert_eq!(by_class(&entries, &classes(&["xterm", "XTerm"])), None);
}

#[test]
fn processes_find_the_entry_whose_command_they_run() {
    let dir = std::env::temp_dir().join(format!("nanocode-computer-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let tool = dir.join("run.py");
    fs::write(&tool, "#!/usr/bin/env python3\n").unwrap();
    let tool = fs::canonicalize(tool).unwrap();
    let tool = tool.to_str().unwrap();
    let entries = [
        listed(
            "second",
            "python3 /harness/fixture.py second %U",
            None,
            "/usr/bin/python3.12",
        ),
        listed("python3.12", "python3.12", None, "/usr/bin/python3.12"),
        listed("gedit-plain", "gedit", None, "/usr/bin/gedit"),
        listed(
            "org.gnome.gedit",
            "gedit --new-window %U",
            None,
            "/usr/bin/gedit",
        ),
        listed("org.example.Tool", &format!("{tool} --gui"), None, tool),
    ];
    let found = |exe: &str, args: &[&str]| by_exec(&entries, &process(exe, args, ""));
    assert_eq!(
        found(
            "/usr/bin/python3.12",
            &["python3", "/harness/fixture.py", "second"]
        )
        .as_deref(),
        Some("second")
    );
    assert_eq!(
        found("/usr/bin/python3.12", &["python3", "/harness/fixture.py"]),
        None
    );
    assert_eq!(found("/usr/bin/python3.12", &["python3.12"]), None);
    assert_eq!(
        found("/usr/bin/gedit", &["gedit", "--new-window", "a.txt"]).as_deref(),
        Some("org.gnome.gedit")
    );
    assert_eq!(
        found("/usr/bin/gedit", &["gedit"]).as_deref(),
        Some("gedit-plain")
    );
    assert_eq!(
        found("/usr/bin/python3.12", &["python3", tool, "--gui"]).as_deref(),
        Some("org.example.Tool")
    );
    assert_eq!(found("/usr/bin/vim", &["vim"]), None);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn an_entry_launches_as_its_desktop_id_and_answers_for_its_program() {
    let xterm = listed(
        "debian-xterm",
        "xterm -class UXTerm",
        None,
        "/usr/bin/xterm",
    )
    .app();
    assert_eq!(
        (xterm.id.as_str(), xterm.aliases.as_slice()),
        ("debian-xterm", &["xterm".to_owned()][..])
    );
    assert_eq!(xterm.path, "/usr/share/applications/debian-xterm.desktop");
    let second = listed(
        "second",
        "python3 /harness/fixture.py second %U",
        None,
        "/usr/bin/python3.12",
    )
    .app();
    assert_eq!(second.id, "second");
    assert_eq!(second.aliases, ["fixture.py", "python3", "python3.12"]);
}
