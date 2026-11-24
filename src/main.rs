#![cfg_attr(windows, windows_subsystem = "windows")]
//! StarCraft II Map Finder (GUI)
//! 배틀넷 캐시에서 맵을 이름으로 검색하고, 맵이 쓰는 모드까지 함께 저장합니다.
//! 캐시 안의 파일은 읽기만 합니다.

mod cache;
mod mpq;

use cache::{Cache, Dep, Progress};
use eframe::egui;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, PartialEq)]
enum Lang {
    Ko,
    En,
    Zh,
}

impl Lang {
    fn detect() -> Lang {
        let l = sys_locale::get_locale().unwrap_or_default().to_lowercase();
        if l.starts_with("ko") {
            Lang::Ko
        } else if l.starts_with("zh") {
            Lang::Zh
        } else {
            Lang::En
        }
    }
    /// 맵 이름을 고를 때 쓸 로캘 순서
    fn locales(self) -> &'static [&'static str] {
        match self {
            Lang::Ko => &["RKok", "SUne"],
            Lang::En => &["SUne", "RKok"],
            Lang::Zh => &["NChz", "WThz", "SUne", "RKok"],
        }
    }
    fn t(self, key: &str) -> &'static str {
        let (ko, en, zh) = match key {
            "title" => ("스타크래프트2 지도 찾기", "StarCraft II Map Finder", "星际争霸2 地图查找"),
            "cache" => ("캐시 폴더", "Cache folder", "缓存文件夹"),
            "reload" => ("다시 읽기", "Reload", "重新读取"),
            "search" => ("맵 이름 검색", "Search map name", "搜索地图名称"),
            "hint" => ("맵 이름 일부를 입력", "Type part of a map name", "输入地图名称的一部分"),
            "clear" => ("지우기", "Clear", "清除"),
            "lang" => ("언어", "Language", "语言"),
            "save_dir" => ("저장 경로", "Save to", "保存位置"),
            "browse" => ("찾아보기...", "Browse...", "浏览..."),
            "as_sc2" => ("SC2Map / SC2Mod 형식으로 저장 (에디터용)", "Save as SC2Map / SC2Mod (for Editor)", "保存为 SC2Map / SC2Mod 格式（编辑器用）"),
            "save_all" => ("맵 + 모드 저장", "Save map + mods", "保存地图 + 模组"),
            "save_mods" => ("모드만 저장", "Save mods only", "仅保存模组"),
            "open_saved" => ("저장한 폴더 열기", "Open saved folder", "打开保存的文件夹"),
            "deps" => ("이 맵이 쓰는 모드", "Mods used by this map", "此地图使用的模组"),
            "pick" => ("왼쪽에서 맵을 고르세요.", "Select a map on the left.", "请在左侧选择地图。"),
            "no_deps" => ("따로 받는 모드가 없습니다 (블리자드 기본 모드만 사용).", "No extra mods (uses only Blizzard mods).", "没有额外模组（仅使用暴雪官方模组）。"),
            "orig" => ("원본 파일", "Original file", "原始文件"),
            "inner" => ("내부 이름", "Internal name", "内部名称"),
            "versions" => ("버전 {}개 중 최신", "Newest of {} versions", "{} 个版本中的最新版"),
            "locate" => ("파일 위치 열기", "Show in folder", "打开文件位置"),
            "missing" => ("캐시에서 찾지 못했습니다 (게임에서 이 맵 방에 한 번 들어가 보세요)", "Not in cache (join a lobby of this map once in game)", "缓存中未找到（请在游戏中进入一次该地图的房间）"),
            "count" => ("맵 {}개 (최근 받은 순)", "{} maps (newest first)", "共 {} 张地图（按最新排序）"),
            "no_cache" => ("캐시를 읽지 못했습니다. 위에서 캐시 폴더를 확인하세요.", "Could not read the cache. Check the cache folder above.", "无法读取缓存，请检查上方的缓存文件夹。"),
            "no_folder" => ("캐시 폴더가 없습니다: ", "Cache folder not found: ", "找不到缓存文件夹："),
            "need_dir" => ("저장 경로를 입력하세요.", "Enter a save location.", "请输入保存位置。"),
            "mkdir_fail" => ("폴더를 만들지 못했습니다: ", "Could not create folder: ", "无法创建文件夹："),
            "saved" => ("파일 {}개를 저장했습니다: ", "Saved {} files: ", "已保存 {} 个文件："),
            "partial" => ("일부 실패: ", "Some failed: ", "部分失败："),
            "list" => ("파일 목록 읽는 중", "Listing files", "正在读取文件列表"),
            "archives" => ("맵·모드 파일 확인 중", "Checking map and mod files", "正在检查地图和模组文件"),
            "info" => ("정보 파일(.s2mh) 읽는 중", "Reading info files (.s2mh)", "正在读取信息文件 (.s2mh)"),
            "done" => ("완료", "Done", "完成"),
            _ => ("?", "?", "?"),
        };
        match self {
            Lang::Ko => ko,
            Lang::En => en,
            Lang::Zh => zh,
        }
    }
}

fn default_cache() -> String {
    let pd = std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".into());
    format!(r"{pd}\Blizzard Entertainment\Battle.net\Cache")
}

fn fmt_time(t: std::time::SystemTime) -> String {
    let dt: chrono::DateTime<chrono::Local> = t.into();
    dt.format("%Y-%m-%d %H:%M").to_string()
}

fn fmt_size(b: u64) -> String {
    if b >= 1024 * 1024 {
        format!("{:.1} MB", b as f64 / 1048576.0)
    } else {
        format!("{} KB", b.div_ceil(1024))
    }
}

fn safe(s: &str) -> String {
    s.chars()
        .map(|c| if r#"\/:*?"<>|"#.contains(c) || c.is_control() { '_' } else { c })
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(windows)]
fn pick_folder(cur: &str) -> Option<String> {
    rfd::FileDialog::new().set_directory(cur).pick_folder().map(|p| p.display().to_string())
}
#[cfg(not(windows))]
fn pick_folder(_cur: &str) -> Option<String> {
    None
}

struct App {
    lang: Lang,
    cache_path: String,
    prog: Arc<Mutex<Progress>>,
    loading: Option<std::thread::JoinHandle<Cache>>,
    cache: Option<Cache>,
    query: String,
    selected: Option<usize>,
    deps: Vec<(Dep, Option<usize>, usize)>,
    message: String,
    last_dir: Option<PathBuf>,
    save_dir: String,
    as_sc2map: bool,
    title_lang: Option<Lang>,
}

impl App {
    fn new(cc: &eframe::CreationContext) -> Self {
        // 한글: 맑은 고딕, 중국어: Microsoft YaHei (윈도우 기본 글꼴)
        let mut fonts = egui::FontDefinitions::default();
        let mut order = vec![];
        for (key, p) in [
            ("kr", r"C:\Windows\Fonts\malgun.ttf"),
            ("zh", r"C:\Windows\Fonts\msyh.ttc"),
            ("zh2", r"C:\Windows\Fonts\simsun.ttc"),
            ("kr_linux", "/usr/share/fonts/truetype/nanum/NanumGothic.ttf"),
        ] {
            if let Ok(data) = std::fs::read(p) {
                fonts.font_data.insert(key.into(), egui::FontData::from_owned(data));
                order.push(key.to_string());
            }
        }
        for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            let list = fonts.families.entry(fam).or_default();
            for (i, k) in order.iter().enumerate() {
                list.insert(i, k.clone());
            }
        }
        cc.egui_ctx.set_fonts(fonts);
        let mut app = App {
            lang: Lang::detect(),
            cache_path: default_cache(),
            prog: Arc::new(Mutex::new(Progress::default())),
            loading: None,
            cache: None,
            query: String::new(),
            selected: None,
            deps: vec![],
            message: String::new(),
            last_dir: None,
            save_dir: dirs::desktop_dir().unwrap_or_else(|| PathBuf::from(".")).display().to_string(),
            as_sc2map: false,
            title_lang: None,
        };
        app.start_load();
        app
    }

    fn t(&self, k: &str) -> &'static str {
        self.lang.t(k)
    }

    fn start_load(&mut self) {
        let root = PathBuf::from(&self.cache_path);
        if !root.is_dir() {
            self.message = format!("{}{}", self.t("no_folder"), self.cache_path);
            return;
        }
        self.cache = None;
        self.selected = None;
        self.deps.clear();
        self.message.clear();
        let prog = self.prog.clone();
        self.loading = Some(std::thread::spawn(move || Cache::load(&root, &prog)));
    }

    fn save(&mut self, with_map: bool) {
        let (Some(c), Some(m)) = (&self.cache, self.selected) else { return };
        let map = &c.docs[m];
        if self.save_dir.trim().is_empty() {
            self.message = self.t("need_dir").into();
            return;
        }
        let map_name = safe(&map.name(self.lang.locales()));
        let dir = PathBuf::from(self.save_dir.trim()).join(format!(
            "{}_{}",
            map_name,
            chrono::DateTime::<chrono::Local>::from(map.mtime).format("%Y%m%d_%H%M")
        ));
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.message = format!("{}{e}", self.t("mkdir_fail"));
            return;
        }
        let mut n = 0;
        let mut errs = vec![];
        let mut copy = |src: &PathBuf, name: String| match std::fs::copy(src, dir.join(&name)) {
            Ok(_) => n += 1,
            Err(e) => errs.push(format!("{name}: {e}")),
        };
        let sc2 = self.as_sc2map;
        if with_map {
            let ext = if sc2 { "SC2Map" } else { "s2ma" };
            copy(&map.path, format!("{map_name}.{ext}"));
        }
        for (i, (dep, hit, _)) in self.deps.iter().enumerate() {
            if let Some(h) = hit {
                let name = if sc2 {
                    // 맵에 적힌 원본 파일 이름 그대로 (에디터가 이 이름으로 찾음)
                    if dep.file.is_empty() { format!("{}.SC2Mod", safe(&dep.bnet_name)) } else { safe(&dep.file) }
                } else {
                    let ext = c.docs[*h].path.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or("s2ma".into());
                    format!("{:02}_{}.{}", i + 1, safe(&dep.bnet_name), ext)
                };
                copy(&c.docs[*h].path, name);
            }
        }
        self.message = format!("{}{}", self.t("saved").replace("{}", &n.to_string()), dir.display());
        if !errs.is_empty() {
            self.message += &format!("\n{}{}", self.t("partial"), errs.join(", "));
        }
        self.last_dir = Some(dir);
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        if self.loading.as_ref().map(|h| h.is_finished()).unwrap_or(false) {
            self.cache = self.loading.take().and_then(|h| h.join().ok());
        }
        if self.loading.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if self.title_lang != Some(self.lang) {
            self.title_lang = Some(self.lang);
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(self.t("title").into()));
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(self.t("cache"));
                ui.add(egui::TextEdit::singleline(&mut self.cache_path).desired_width(420.0));
                if ui.add_enabled(self.loading.is_none(), egui::Button::new(self.t("reload"))).clicked() {
                    self.start_load();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    egui::ComboBox::from_id_salt("lang")
                        .selected_text(match self.lang {
                            Lang::Ko => "한국어",
                            Lang::En => "English",
                            Lang::Zh => "中文",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.lang, Lang::Ko, "한국어");
                            ui.selectable_value(&mut self.lang, Lang::En, "English");
                            ui.selectable_value(&mut self.lang, Lang::Zh, "中文");
                        });
                    ui.label(self.t("lang"));
                });
            });
            ui.horizontal(|ui| {
                ui.label(self.t("search"));
                ui.add(egui::TextEdit::singleline(&mut self.query).desired_width(420.0).hint_text(self.lang.t("hint")));
                if ui.button(self.t("clear")).clicked() {
                    self.query.clear();
                }
            });
            ui.add_space(6.0);
        });

        egui::TopBottomPanel::bottom("bottom").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(self.t("save_dir"));
                ui.add(egui::TextEdit::singleline(&mut self.save_dir).desired_width(420.0));
                if ui.button(self.t("browse")).clicked() {
                    if let Some(p) = pick_folder(&self.save_dir) {
                        self.save_dir = p;
                    }
                }
                ui.checkbox(&mut self.as_sc2map, self.lang.t("as_sc2"));
            });
            ui.horizontal(|ui| {
                let can = self.selected.is_some();
                if ui.add_enabled(can, egui::Button::new(self.t("save_all"))).clicked() {
                    self.save(true);
                }
                if ui.add_enabled(can, egui::Button::new(self.t("save_mods"))).clicked() {
                    self.save(false);
                }
                if let Some(d) = &self.last_dir {
                    if ui.button(self.t("open_saved")).clicked() {
                        let _ = std::process::Command::new("explorer.exe").arg(d).spawn();
                    }
                }
            });
            if !self.message.is_empty() {
                ui.label(&self.message);
            }
            ui.add_space(4.0);
        });

        let lang = self.lang;
        // 오른쪽: 선택한 맵의 모드
        egui::SidePanel::right("deps").min_width(380.0).show(ctx, |ui| {
            ui.heading(lang.t("deps"));
            ui.separator();
            let Some(c) = &self.cache else { return };
            let Some(m) = self.selected else {
                ui.label(lang.t("pick"));
                return;
            };
            ui.label(egui::RichText::new(c.docs[m].name(lang.locales())).strong());
            ui.small(c.docs[m].path.display().to_string());
            ui.add_space(6.0);
            if self.deps.is_empty() {
                ui.label(lang.t("no_deps"));
            }
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (i, (dep, hit, n)) in self.deps.iter().enumerate() {
                    ui.group(|ui| {
                        ui.set_min_width(ui.available_width());
                        ui.label(egui::RichText::new(format!("{}. {}", i + 1, dep.bnet_name)).strong());
                        if !dep.file.is_empty() {
                            ui.small(format!("{}: {}", lang.t("orig"), dep.file));
                        }
                        match hit {
                            Some(h) => {
                                let d = &c.docs[*h];
                                ui.label(format!("{}: {}   {}   {}", lang.t("inner"), d.name(lang.locales()), fmt_time(d.mtime), fmt_size(d.size)));
                                if *n > 1 {
                                    ui.small(lang.t("versions").replace("{}", &n.to_string()));
                                }
                                if ui.small_button(lang.t("locate")).clicked() {
                                    let _ = std::process::Command::new("explorer.exe").arg(format!("/select,{}", d.path.display())).spawn();
                                }
                            }
                            None => {
                                ui.colored_label(egui::Color32::from_rgb(220, 150, 0), lang.t("missing"));
                            }
                        }
                    });
                }
            });
        });

        // 가운데: 맵 목록
        egui::CentralPanel::default().show(ctx, |ui| {
            if self.loading.is_some() {
                let p = self.prog.lock().unwrap();
                ui.label(format!("{} ...", lang.t(&p.stage)));
                if p.total > 0 {
                    ui.add(egui::ProgressBar::new(p.done as f32 / p.total as f32).text(format!("{} / {}", p.done, p.total)));
                }
                return;
            }
            let Some(c) = &self.cache else {
                ui.label(lang.t("no_cache"));
                return;
            };
            let q = self.query.to_lowercase();
            let mut list: Vec<usize> = (0..c.docs.len())
                .filter(|&i| c.docs[i].is_map && (q.is_empty() || c.docs[i].matches(&q)))
                .collect();
            list.sort_by(|a, b| c.docs[*b].mtime.cmp(&c.docs[*a].mtime));
            ui.label(lang.t("count").replace("{}", &list.len().to_string()));
            ui.separator();
            let mut clicked = None;
            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Grid::new("maps").striped(true).num_columns(3).show(ui, |ui| {
                    for i in list {
                        let d = &c.docs[i];
                        if ui.selectable_label(self.selected == Some(i), d.name(lang.locales())).clicked() {
                            clicked = Some(i);
                        }
                        ui.label(fmt_time(d.mtime));
                        ui.label(fmt_size(d.size));
                        ui.end_row();
                    }
                });
            });
            if let Some(i) = clicked {
                self.selected = Some(i);
                self.deps = c.resolve(i);
                self.message.clear();
            }
        });
    }
}

fn main() -> eframe::Result<()> {
    let title = Lang::detect().t("title");
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1150.0, 700.0]).with_title(title),
        ..Default::default()
    };
    eframe::run_native("sc2-map-finder", opts, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
