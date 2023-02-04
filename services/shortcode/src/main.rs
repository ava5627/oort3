use anyhow::{anyhow, bail};
use firestore::*;
use gcloud_sdk::google::firestore::v1::Document;
use oort_proto::LeaderboardSubmission;
use regex::Regex;
use salvo::prelude::*;
use salvo_extra::cors::Cors;

fn project_id() -> &'static str {
    match std::env::var("ENVIRONMENT") {
        Ok(x) if x == "dev" => "oort-dev",
        Ok(x) if x == "prod" => "oort-319301",
        _ => {
            panic!("Invalid ENVIRONMENT")
        }
    }
}

#[derive(Clone, Debug)]
enum Shortcode {
    Leaderboard {
        username: String,
        scenario_name: String,
    },
}

fn parse_id(id: &str) -> anyhow::Result<Shortcode> {
    let leaderboard_re = Regex::new(r"^leaderboard:([a-zA-A0-9_-]+):(\w+)$")?;
    if let Some(caps) = leaderboard_re.captures(id) {
        let username = caps.get(1).unwrap().as_str().to_string();
        let scenario_name = caps.get(2).unwrap().as_str().to_string();
        Ok(Shortcode::Leaderboard {
            username,
            scenario_name,
        })
    } else {
        bail!("id did not match any known formats")
    }
}

async fn fetch_leaderboard(
    db: &FirestoreDb,
    scenario_name: &str,
    username: &str,
) -> anyhow::Result<String> {
    let docs: Vec<Document> = db
        .query_doc(
            FirestoreQueryParams::new("leaderboard".into())
                .with_filter(FirestoreQueryFilter::Composite(
                    FirestoreQueryFilterComposite::new(vec![
                        FirestoreQueryFilter::Compare(Some(FirestoreQueryFilterCompare::Equal(
                            "scenario_name".into(),
                            scenario_name.into(),
                        ))),
                        FirestoreQueryFilter::Compare(Some(FirestoreQueryFilterCompare::Equal(
                            "username".into(),
                            username.into(),
                        ))),
                    ]),
                ))
                .with_order_by(vec![
                    FirestoreQueryOrder::new("time".to_owned(), FirestoreQueryDirection::Ascending),
                    FirestoreQueryOrder::new(
                        "timestamp".to_owned(),
                        FirestoreQueryDirection::Ascending,
                    ),
                ])
                .with_limit(1),
        )
        .await?;

    for doc in &docs {
        if let Ok(msg) = FirestoreDb::deserialize_doc_to::<LeaderboardSubmission>(doc) {
            return oort_code_encryption::encrypt(&msg.code);
        }
    }

    bail!("no matching leaderboard entry found");
}

async fn get_shortcode_internal(req: &mut Request, res: &mut Response) -> anyhow::Result<()> {
    let db = FirestoreDb::new(project_id()).await?;
    log::debug!("Got request {:?}", req);
    let id: String = req.param("id").ok_or(anyhow!("missing id parameter"))?;
    let code = match parse_id(&id)? {
        Shortcode::Leaderboard {
            username,
            scenario_name,
        } => fetch_leaderboard(&db, &scenario_name, &username).await?,
    };

    res.render(code);
    Ok(())
}

#[handler]
async fn get_shortcode(req: &mut Request, res: &mut Response) {
    if let Err(e) = get_shortcode_internal(req, res).await {
        log::error!("error: {}", e);
        res.set_status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(e.to_string());
    }
}

#[handler]
async fn nop(_req: &mut Request, res: &mut Response) {
    res.render("");
}

#[tokio::main]
pub async fn main() {
    stackdriver_logger::init_with_cargo!();

    let mut port: u16 = 8084;
    match std::env::var("PORT") {
        Ok(p) => {
            match p.parse::<u16>() {
                Ok(n) => {
                    port = n;
                }
                Err(_e) => {}
            };
        }
        Err(_e) => {}
    };

    log::info!("Starting oort_shortcode_service");
    log::info!("Using project ID {}", project_id());

    let cors_handler = Cors::builder()
        .allow_any_origin()
        .allow_methods(vec!["POST", "OPTIONS"])
        .allow_header("content-type")
        .build();

    let router = Router::with_hoop(cors_handler).push(
        Router::with_path("/shortcode/<id>")
            .get(get_shortcode)
            .options(nop),
    );

    Server::new(TcpListener::bind(&format!("0.0.0.0:{port}")))
        .serve(router)
        .await;
}
