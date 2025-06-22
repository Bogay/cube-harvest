use askama::Template;
use core::panic;
use k8s_openapi::api::core::v1::Node;
use k8s_openapi::api::core::v1::Pod;
use kube::api::PostParams;
use kube::{Api, Client, Config, api::ListParams};
use macroquad::experimental::collections::storage;
use macroquad::prelude::coroutines::start_coroutine;
use macroquad::prelude::coroutines::wait_seconds;
use macroquad::prelude::*;
use macroquad_particles::{self, AtlasConfig, Emitter, EmitterConfig};
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tokio::task::JoinHandle;

const MOVEMENT_SPEED: f32 = 200.;
const FRAGMENT_SHADER: &str = include_str!("starfield-shader.glsl");
const VERTEX_SHADER: &str = "#version 100

attribute vec3 position;
attribute vec2 texcoord;
attribute vec4 color0;
varying float iTime;

uniform mat4 Model;
uniform mat4 Projection;
uniform vec4 _Time;

void main() {
    gl_Position = Projection * Model * vec4(position, 1);
    iTime = _Time.x;
}
";

enum GameStage {
    MainMenu,
    Playing,
    Paused,
    GameOver,
}

struct Shape {
    size: f32,
    speed: f32,
    x: f32,
    y: f32,
    collided: bool,
}

#[derive(Template, Debug)]
#[template(path = "astro-unit.json", escape = "none")]
struct AstroUnitTemplate {
    name: String,
    target_ip: String,
    unit_type: String,
}

struct GameResources {
    pods: Vec<Pod>,
    nodes: Vec<Node>,
}

impl GameResources {
    // TODO: error handling
    pub async fn new(client: &Client) -> Self {
        let list_params = ListParams::default();
        let pods = Api::default_namespaced(client.clone())
            .list(&list_params)
            .await
            .expect("failed to get pods");
        let nodes = Api::all(client.clone())
            .list(&list_params)
            .await
            .expect("failed to get nodes");

        Self {
            pods: pods.items,
            nodes: nodes.items,
        }
    }
}

#[derive(Debug, Clone)]
enum NavigationMode {
    Cluster,
    Node,
    Create,
}

#[derive(Debug, Clone)]
enum CreateTarget {
    Miner,
    Processor,
}

#[derive(Debug, Clone)]
struct GameState {
    selected_node_index: usize,
    navigation_mode: NavigationMode,
    create_target: Option<CreateTarget>,
    create_text_buf: String,
    credits: usize,
    miner_price: usize,
    processor_price: usize,
}

impl Shape {
    fn collides_with(&self, other: &Self) -> bool {
        self.rect().overlaps(&other.rect())
    }

    fn rect(&self) -> Rect {
        Rect {
            x: self.x - self.size / 2.,
            y: self.y - self.size / 2.,
            w: self.size,
            h: self.size,
        }
    }
}

#[tokio::main]
async fn main() {
    // setup kube client
    let config = Config::infer().await.expect("failed to load kubeconfig");
    let client = Client::try_from(config).expect("failed to create kube client");
    let game_resources = GameResources::new(&client).await;
    let (tx, rx) = mpsc::channel(0x20);
    tx.send(GameMessage::UpdateResources(game_resources))
        .await
        .expect("failed to send game msg");
    let (k_tx, mut k_rx) = mpsc::channel(0x20);

    // TODO: handle exiting game
    let reconciliation_loop = tokio::spawn(async move {
        loop {
            let game_resources = GameResources::new(&client).await;
            tx.send(GameMessage::UpdateResources(game_resources))
                .await
                .expect("failed to send game msg");
            match k_rx.try_recv() {
                Ok(msg) => match msg {
                    GameMessage::CreatePod(pod) => {
                        let api = Api::default_namespaced(client.clone());
                        api.create(&PostParams::default(), &pod)
                            .await
                            .expect("failed to create pod");
                    }
                    GameMessage::UpdateResources(_) => unreachable!(),
                },
                Err(err) => {
                    if !matches!(err, mpsc::error::TryRecvError::Empty) {
                        panic!("{err}");
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });

    // Because macroquad need to be executed on one thread, we open it
    // from tokio main function
    // ref: https://github.com/not-fl3/macroquad/issues/182#issuecomment-1001571263
    let game_window_handle = open_game_window(rx, k_tx);

    game_window_handle.await.unwrap();
    reconciliation_loop.await.unwrap();
}

enum GameMessage {
    UpdateResources(GameResources),
    CreatePod(Pod),
}

fn open_game_window(rx: Receiver<GameMessage>, k_tx: Sender<GameMessage>) -> JoinHandle<()> {
    tokio::task::spawn_blocking(|| {
        macroquad::Window::from_config(
            Conf {
                sample_count: 4,
                window_title: "CubeHarvest: Cluster Frontier".to_string(),
                high_dpi: true,
                ..Default::default()
            },
            draw(rx, k_tx),
        );
    })
}

async fn draw(mut rx: Receiver<GameMessage>, k_tx: Sender<GameMessage>) {
    rand::srand(miniquad::date::now() as u64);
    set_pc_assets_folder("assets");

    storage::store(GameState {
        selected_node_index: 0,
        navigation_mode: NavigationMode::Cluster,
        create_target: None,
        create_text_buf: "".to_string(),
        credits: 0,
        miner_price: 0,
        processor_price: 0,
    });

    let mut explosions: Vec<(Emitter, Vec2)> = vec![];
    let mut game_stage = GameStage::MainMenu;
    let mut squares: Vec<Shape> = vec![];
    let mut bullets: Vec<Shape> = vec![];
    let mut circle = Shape {
        size: 32.,
        speed: MOVEMENT_SPEED,
        x: screen_width() / 2.,
        y: screen_height() / 2.,
        collided: false,
    };
    let render_target = render_target(320, 150);
    render_target.texture.set_filter(FilterMode::Nearest);
    let material = load_material(
        ShaderSource::Glsl {
            vertex: VERTEX_SHADER,
            fragment: FRAGMENT_SHADER,
        },
        MaterialParams {
            uniforms: vec![
                UniformDesc::new("iResolution", UniformType::Float2),
                UniformDesc::new("direction_modifier", UniformType::Float1),
            ],
            ..Default::default()
        },
    )
    .unwrap();
    // call after loading all textures
    build_textures_atlas();

    // game loop
    loop {
        clear_background(BLACK);

        // consume messages
        loop {
            match rx.try_recv() {
                Ok(msg) => match msg {
                    GameMessage::UpdateResources(game_resources) => storage::store(game_resources),
                    GameMessage::CreatePod(_) => unreachable!(),
                },
                Err(err) => {
                    if matches!(err, mpsc::error::TryRecvError::Empty) {
                        break;
                    }
                    panic!("{err}");
                }
            }
        }

        {
            let mut game_state = storage::get::<GameState>().clone();
            game_state.miner_price = storage::get::<GameResources>()
                .pods
                .iter()
                .filter(|p| matches!(get_unit_type(p).as_deref(), Some("miner")))
                .count();
            game_state.processor_price = storage::get::<GameResources>()
                .pods
                .iter()
                .filter(|p| matches!(get_unit_type(p).as_deref(), Some("processor")))
                .count();
            storage::store(game_state);
        }

        material.set_uniform("iResolution", (screen_width(), screen_height()));
        gl_use_material(&material);
        draw_texture_ex(
            &render_target.texture,
            0.,
            0.,
            WHITE,
            DrawTextureParams {
                dest_size: Some(vec2(screen_width(), screen_height())),
                ..Default::default()
            },
        );
        gl_use_default_material();

        match game_stage {
            GameStage::MainMenu => {
                // update
                if is_key_pressed(KeyCode::Escape) {
                    std::process::exit(0);
                }

                if is_key_pressed(KeyCode::Space) {
                    squares.clear();
                    bullets.clear();
                    explosions.clear();
                    circle.x = screen_width() / 2.;
                    circle.y = screen_height() / 2.;
                    game_stage = GameStage::Playing;
                    start_update_credits();
                }

                // draw
                let text = "Press space";
                let text_dimestions = measure_text(text, None, 50, 1.);
                draw_text(
                    text,
                    screen_width() / 2. - text_dimestions.width / 2.,
                    screen_height() / 2.,
                    50.,
                    WHITE,
                );
            }
            GameStage::Playing => {
                // update
                let delta_time = get_frame_time();
                let mut game_state = storage::get_mut::<GameState>().clone();
                let nodes_len = {
                    let game_resources = storage::get::<GameResources>();
                    game_resources.nodes.len()
                };

                match game_state.navigation_mode {
                    NavigationMode::Cluster => {
                        if is_key_pressed(KeyCode::Right) {
                            game_state.selected_node_index =
                                game_state.selected_node_index.saturating_add(1);
                        }
                        if is_key_pressed(KeyCode::Left) {
                            game_state.selected_node_index =
                                game_state.selected_node_index.saturating_sub(1);
                        }
                        if is_key_pressed(KeyCode::Enter) {
                            game_state.navigation_mode = NavigationMode::Node;
                        }
                        if is_key_pressed(KeyCode::C) {
                            game_state.navigation_mode = NavigationMode::Create;
                            game_state.create_text_buf.clear();
                            game_state.create_target = None;
                        }
                    }
                    NavigationMode::Node => {
                        if is_key_pressed(KeyCode::Escape) {
                            game_state.navigation_mode = NavigationMode::Cluster;
                        }

                        if is_key_pressed(KeyCode::D) {
                            // TODO: delete selected unit
                        }
                        if is_key_pressed(KeyCode::Right) {
                            // TODO: update unit selection
                        }
                        if is_key_pressed(KeyCode::Left) {
                            // TODO: update unit selection
                        }
                    }
                    NavigationMode::Create => match &game_state.create_target {
                        None => {
                            if is_key_pressed(KeyCode::Escape) {
                                game_state.navigation_mode = NavigationMode::Cluster;
                            }

                            if is_key_pressed(KeyCode::M) {
                                game_state.create_target = Some(CreateTarget::Miner);
                            }
                            if is_key_pressed(KeyCode::P) {
                                game_state.create_target = Some(CreateTarget::Processor);
                            }
                        }
                        Some(target) => {
                            if is_key_pressed(KeyCode::Enter)
                                || matches!(target, CreateTarget::Processor)
                            {
                                let has_enough_credit = match target {
                                    CreateTarget::Miner => {
                                        game_state.credits >= game_state.miner_price
                                    }
                                    CreateTarget::Processor => {
                                        game_state.credits >= game_state.processor_price
                                    }
                                };

                                if has_enough_credit {
                                    let astro_unit = create_unit(&game_state, target);
                                    println!("Create {target:?} -> {}", game_state.create_text_buf);
                                    k_tx.send(GameMessage::CreatePod(astro_unit))
                                        .await
                                        .expect("failed to send pod");
                                    match target {
                                        CreateTarget::Miner => {
                                            game_state.credits -= game_state.miner_price;
                                        }
                                        CreateTarget::Processor => {
                                            game_state.credits -= game_state.processor_price;
                                        }
                                    }
                                } else {
                                    // TODO: alert
                                }

                                game_state.navigation_mode = NavigationMode::Cluster;
                            } else if is_key_pressed(KeyCode::Escape) {
                                game_state.navigation_mode = NavigationMode::Cluster;
                            } else if is_key_pressed(KeyCode::Backspace) {
                                game_state.create_text_buf.pop();
                            } else if let Some(c) = get_char_pressed() {
                                if c.is_digit(10) || c == '.' {
                                    game_state.create_text_buf.push(c);
                                }
                            }
                        }
                    },
                }

                // if is_key_down(KeyCode::Down) {
                //     circle.y += MOVEMENT_SPEED * delta_time;
                // }
                // if is_key_down(KeyCode::Up) {
                //     circle.y -= MOVEMENT_SPEED * delta_time;
                // }
                // circle.x = clamp(circle.x, 0.0, screen_width());
                // circle.y = clamp(circle.y, 0.0, screen_height());

                // for sq in &mut squares {
                //     for bullet in &mut bullets {
                //         if bullet.collides_with(sq) {
                //             bullet.collided = true;
                //             sq.collided = true;

                //             score += sq.size.round() as u32;
                //             high_score = high_score.max(score);

                //             explosions.push((
                //                 Emitter::new(EmitterConfig {
                //                     amount: sq.size.round() as u32 * 2,
                //                     texture: Some(explosion_texture.clone()),
                //                     ..particle_explosion()
                //                 }),
                //                 vec2(sq.x, sq.y),
                //             ));
                //         }
                //     }
                // }

                // if squares.iter().any(|s| circle.collides_with(s)) {
                //     if score == high_score {
                //         fs::write("highscore.dat", high_score.to_string())
                //             .expect("failed to record high score.");
                //     }
                //     game_state = GameState::GameOver;
                // }
                game_state.selected_node_index =
                    clamp(game_state.selected_node_index, 0, nodes_len - 1);

                // post update
                storage::store(game_state);

                // draw enemy
                // enemy_small_sprite.update();
                // let enemy_frame = enemy_small_sprite.frame();
                // for sq in &squares {
                //     draw_texture_ex(
                //         &enemy_small_texture,
                //         sq.x - sq.size / 2.,
                //         sq.y - sq.size / 2.,
                //         WHITE,
                //         DrawTextureParams {
                //             dest_size: Some(vec2(sq.size, sq.size)),
                //             source: Some(enemy_frame.source_rect),
                //             ..Default::default()
                //         },
                //     );
                // }
                // for (e, coords) in &mut explosions {
                //     e.draw(*coords);
                // }
                // draw spaceship
                // ship_sprite.update();
                // let ship_frame = ship_sprite.frame();
                // draw_texture_ex(
                //     &ship_texture,
                //     circle.x - circle.size / 2.,
                //     circle.y - circle.size / 2.,
                //     WHITE,
                //     DrawTextureParams {
                //         dest_size: Some(vec2(circle.size, circle.size)),
                //         source: Some(ship_frame.source_rect),
                //         ..Default::default()
                //     },
                // );
                // draw bullets
                // bullet_sprite.update();
                // let bullet_frame = bullet_sprite.frame();
                // for bullet in &bullets {
                //     draw_texture_ex(
                //         &bullet_texture,
                //         bullet.x - bullet.size / 2.,
                //         bullet.y - bullet.size / 2.,
                //         WHITE,
                //         DrawTextureParams {
                //             dest_size: Some(vec2(bullet.size, bullet.size)),
                //             source: Some(bullet_frame.source_rect),
                //             ..Default::default()
                //         },
                //     );
                // }
                draw_top_panel();

                draw_node();
                draw_navbar();
            }
            GameStage::Paused => {
                if is_key_pressed(KeyCode::Space) {
                    game_stage = GameStage::Playing;
                }

                let text = "Paused";
                let text_dimensions = measure_text(text, None, 50, 1.);
                draw_text(
                    text,
                    screen_width() / 2. - text_dimensions.width / 2.,
                    screen_height() / 2.,
                    50.,
                    WHITE,
                );
            }
            GameStage::GameOver => {
                if is_key_pressed(KeyCode::Space) {
                    game_stage = GameStage::MainMenu;
                }

                let text = "GAME OVER!";
                let text_dimensions = measure_text(text, None, 50, 1.);
                draw_text(
                    text,
                    screen_width() / 2. - text_dimensions.width / 2.,
                    screen_height() / 2.,
                    50.,
                    RED,
                );
            }
        };

        next_frame().await
    }
}

fn create_unit(game_state: &GameState, target: &CreateTarget) -> Pod {
    let unit_id = rand::rand();
    let unit_type = match target {
        CreateTarget::Miner => "miner",
        CreateTarget::Processor => "processor",
    }
    .to_string();
    let astro_unit = AstroUnitTemplate {
        name: format!("{unit_type}-{unit_id}"),
        target_ip: game_state.create_text_buf.clone(),
        unit_type,
    }
    .render()
    .unwrap();
    let astro_unit =
        serde_json::from_str::<Pod>(&astro_unit).expect("failed to parse astro unit json");
    astro_unit
}

fn start_update_credits() {
    start_coroutine(earn_credits());
    start_coroutine(consume_credits());
}

async fn earn_credits() {
    loop {
        {
            let earned_credits = {
                let mut m = HashMap::new();
                let game_resources = storage::get::<GameResources>();
                for p in &game_resources.pods {
                    if matches!(get_unit_type(p).as_deref(), Some("processor")) {
                        let Some(ip) = get_unit_ip(p).to_owned() else {
                            continue;
                        };
                        m.insert(ip, 0);
                    }
                }

                for p in &game_resources.pods {
                    if matches!(get_unit_type(p).as_deref(), Some("miner")) {
                        let Some(target_ip) = p
                            .spec
                            .as_ref()
                            .and_then(|s| s.containers[0].env.as_ref())
                            .and_then(|e| e.iter().find(|e| e.name == "TARGET"))
                            .and_then(|e| e.value.clone())
                        else {
                            continue;
                        };
                        if let Some(c) = m.get_mut(target_ip.as_str()) {
                            *c += 1;
                        }
                    }
                }

                m.into_values().map(|x| x.min(3)).sum::<usize>()
            };
            {
                let mut game_state = storage::get_mut::<GameState>();
                game_state.credits = game_state.credits.saturating_add(earned_credits);
            }
        }
        wait_seconds(1.).await;
    }
}

async fn consume_credits() {
    loop {
        {
            let consumed_credits = storage::get::<GameResources>().pods.len();
            {
                let mut game_state = storage::get_mut::<GameState>();
                game_state.credits = game_state.credits.saturating_sub(consumed_credits);
            }
        }
        wait_seconds(3.).await;
    }
}

fn get_unit_ip(p: &Pod) -> Option<&str> {
    p.status.as_ref().and_then(|s| s.pod_ip.as_deref())
}

fn draw_top_panel() {
    let game_state = storage::get::<GameState>().clone();
    let game_resources = storage::get::<GameResources>();

    let label_size = 25;
    let label_scale = 1.0;
    let label_padding = 4.0;
    let label_dimensions = measure_text("Placeholder", None, label_size, label_scale);
    draw_text(
        &format!("Astro Units: {}", game_resources.pods.len()),
        10.0,
        35.0,
        label_size as f32,
        WHITE,
    );
    draw_text(
        &format!("Credits    : {}", game_state.credits),
        10.0,
        35.0 + (label_dimensions.height + label_padding) * 2.,
        label_size as f32,
        WHITE,
    );
    draw_text(
        &format!("Astro Node : {}", game_state.selected_node_index),
        10.0,
        35.0 + label_dimensions.height + label_padding,
        label_size as f32,
        WHITE,
    );
}

fn draw_node() {
    let width = screen_width();
    let height = screen_height();
    let node_index = storage::get::<GameState>().selected_node_index;
    let game_resources = storage::get::<GameResources>();
    let node = &game_resources.nodes[node_index];
    let node_name = node.metadata.name.as_ref().expect("nodes should have name");
    let pods = game_resources
        .pods
        .iter()
        .filter(|p| {
            p.spec
                .as_ref()
                .and_then(|s| s.node_name.as_ref())
                .map(|nn| nn == node_name)
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();

    // draw node plane
    let node_width = width * 0.7;
    let node_height = 100.;
    draw_rectangle(
        width / 2. - node_width / 2.,
        height - node_height / 2.,
        node_width,
        node_height,
        WHITE,
    );

    // draw pods info
    let pod_size = 32.;
    let gap = pod_size * 3.;
    for (i, p) in pods.iter().enumerate() {
        match get_unit_type(p).as_deref() {
            Some("miner") => {
                draw_miner(
                    p,
                    width / 2. - 200. + gap * i as f32,
                    height - node_height / 2. + 15. - pod_size / 2.,
                    pod_size,
                    BLUE,
                );
            }
            _ => {
                draw_processor(
                    p,
                    width / 2. - 200. + gap * i as f32,
                    height - node_height / 2. + 15. - pod_size / 2. - 48.,
                    pod_size,
                    PINK,
                );
            }
        }
    }
    // draw_text(&format!("{}", pods.len()), 0., height - 10., 18., WHITE);
}

fn get_unit_type(p: &Pod) -> Option<String> {
    p.metadata
        .labels
        .as_ref()
        .and_then(|l| l.get("cube-harvest.io/unit-type").cloned())
}

fn draw_miner(pod: &Pod, x: f32, y: f32, size: f32, color: Color) {
    // Main body (simple rectangle or custom polygon)
    draw_rectangle(x - size / 2.0, y - size / 2.0, size, size, color);

    // a small "engine" or "sensor" part
    draw_triangle(
        vec2(x - size / 4.0, y + size / 2.0),
        vec2(x + size / 4.0, y + size / 2.0),
        vec2(x, y + size / 2.0 + size / 4.0),
        GRAY,
    );

    if let Some(ip) = get_unit_ip(pod) {
        draw_text(&ip.to_string(), x - size / 2.0, y, 18., WHITE);
    }
}

fn draw_processor(pod: &Pod, x: f32, y: f32, size: f32, color: Color) {
    // Main body (simple rectangle or custom polygon)
    draw_rectangle(x - size / 2.0, y - size / 2.0, size, size, color);

    // a small "engine" or "sensor" part
    draw_triangle(
        vec2(x - size / 4.0, y - size / 2.0),
        vec2(x + size / 4.0, y - size / 2.0),
        vec2(x, y - size / 2.0 - size / 4.0),
        GRAY,
    );

    if let Some(ip) = get_unit_ip(pod) {
        draw_text(&ip.to_string(), x - size / 2.0, y, 18., WHITE);
    }
}

fn draw_navbar() {
    let width = screen_width();
    let height = screen_height();
    let navigation_mode = storage::get::<GameState>().navigation_mode.clone();

    let label_font_size = 18;
    let label_dim = measure_text("Cluster", None, label_font_size, 1.);
    let padding = 4.;

    // draw navbar background
    draw_rectangle(
        0.,
        height - padding * 2. - label_dim.height - 5.,
        width,
        padding * 2. + label_dim.height + 10.,
        GRAY,
    );

    // draw tooltip
    let mut tooltip = String::with_capacity(0x50);
    match navigation_mode {
        NavigationMode::Cluster => {
            tooltip.push_str("Cluster");
            tooltip.push_str(" | [Enter] Select node");
            tooltip.push_str(" | [<- ->] Switch node");
            tooltip.push_str(" | [C]reate unit");
        }
        NavigationMode::Node => {
            tooltip.push_str("Node   ");
            tooltip.push_str(" | [Esc] Back");
            tooltip.push_str(" | [<- ->] Switch unit");
            tooltip.push_str(" | [D]elete unit");
        }
        NavigationMode::Create => {
            tooltip.push_str("Create ");
            let game_state = storage::get::<GameState>();
            match game_state.create_target.as_ref() {
                Some(target) => {
                    tooltip.push_str(" | ");
                    tooltip.push_str(&format!("{target:?}"));
                    tooltip.push_str(" : ");
                    tooltip.push_str(&game_state.create_text_buf);
                }
                None => {
                    tooltip.push_str(" | [Esc] Back");
                    tooltip.push_str(&format!(" | [M]iner (${})", game_state.miner_price));
                    tooltip.push_str(&format!(" | [P]rocessor (${})", game_state.processor_price));
                }
            }
        }
    }
    draw_text(
        &tooltip,
        0. + padding,
        height - label_dim.height / 2. - padding,
        18.,
        WHITE,
    );
}
