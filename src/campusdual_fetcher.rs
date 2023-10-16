use scraper::{Html, Selector};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncWriteExt},
};

use crate::data_types::stuwe_data_types::{CampusDualError, CampusDualGrade};

async fn extract_data(html_text: String) -> Vec<CampusDualGrade> {
    let mut grades = Vec::new();

    let document = Html::parse_document(&html_text);
    let table = document
        .select(&Selector::parse("#acwork tbody").unwrap())
        .next()
        .unwrap();
    let top_level_line_selector = Selector::parse(".child-of-node-0").unwrap();
    let top_level_lines = table.select(&top_level_line_selector);
    for line in top_level_lines {
        let l_id = line.value().attr("id").unwrap();
        let content_selector = &Selector::parse("td").unwrap();
        let mut content = line.select(content_selector);
        let name = content.next().unwrap().text().next().unwrap();
        let grade = content.next().unwrap().text().next().unwrap(); // .inner_html();

        let subline_selector = &Selector::parse(&format!(".child-of-{}", l_id)).unwrap();
        let sub_count = table.select(subline_selector).count();

        grades.push(CampusDualGrade {
            class: name.to_string(),
            grade: grade.to_string(),
            subgrades: sub_count,
        });
    }

    grades
}

pub async fn get_campusdual_grades(
    uname: String,
    password: String,
) -> Result<Vec<CampusDualGrade>, Box<dyn std::error::Error + Sync + Send>> {
    let client = reqwest::Client::builder().cookie_store(true).build()?;

    let resp = client
        .get("https://erp.campus-dual.de/sap/bc/webdynpro/sap/zba_initss?sap-client=100&sap-language=de&uri=https://selfservice.campus-dual.de/index/login")
        .send()
        .await?;

    if resp.status() != 200 {
        return Err(CampusDualError::CdInitFailed(resp.status().as_u16()).into());
    }

    let xsrf = {
        let document = Html::parse_document(&resp.text().await.unwrap());
        document
            .select(&Selector::parse(r#"input[name="sap-login-XSRF"]"#).unwrap())
            .next()
            .unwrap()
            .value()
            .attr("value")
            .unwrap()
            .to_string()
    };

    let form = [
        ("sap-user", uname),
        ("sap-password", password),
        ("sap-login-XSRF", xsrf),
    ];

    let resp = client
        .post("https://erp.campus-dual.de/sap/bc/webdynpro/sap/zba_initss?uri=https%3a%2f%2fselfservice.campus-dual.de%2findex%2flogin&sap-client=100&sap-language=DE")
        .form(&form)
        .header("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/117.0.0.0 Safari/537.36")
        .send()
        .await
        .unwrap();

    if resp.status() != 200 {
        return Err(CampusDualError::CdZbaFailed(resp.status().as_u16()).into());
    }

    // check if title of redirect page implicates successful login
    {
        let zba_init_doc = Html::parse_document(&resp.text().await.unwrap());
        match zba_init_doc
            .select(&Selector::parse("title").unwrap())
            .next()
            .unwrap()
            .inner_html()
            .as_str()
        {
            "Anmeldung" => return Err(CampusDualError::CdBadCredentials.into()),
            "Initialisierung Selfservices" => {}
            _ => log::warn!("Unexpected redirect, treating as success"),
        }
    }

    let resp = client
        .get("https://selfservice.campus-dual.de/acwork/index")
        .send()
        .await
        .unwrap();
    log::debug!("get exams: {}", resp.status());

    Ok(extract_data(resp.text().await.unwrap()).await)
}

pub async fn compare_campusdual_grades(
    recv_grades: &Vec<CampusDualGrade>,
) -> Option<Vec<CampusDualGrade>> {
    let mut f = match File::open("grades.json").await {
        Ok(f) => f,
        Err(_) => {
            let mut f = File::create("grades.json").await.unwrap();
            let json_str = serde_json::to_string(recv_grades).unwrap();
            f.write_all(json_str.as_bytes()).await.unwrap();
            return Some(recv_grades.to_vec());
        }
    };
    let mut new_grades = vec![];

    let mut old_grades_str = String::new();
    f.read_to_string(&mut old_grades_str).await.unwrap();
    let old_grades: Vec<CampusDualGrade> = serde_json::from_str(&old_grades_str).unwrap();

    for grade in recv_grades {
        if !old_grades.contains(grade) {
            new_grades.push(grade.clone());
        }
    }

    if new_grades.is_empty() {
        None
    } else {
        Some(new_grades)
    }
}

pub async fn save_campusdual_grades(recv_grades: &Vec<CampusDualGrade>) {
    let mut f = File::create("grades.json").await.unwrap();
    let json_str = serde_json::to_string(recv_grades).unwrap();
    f.write_all(json_str.as_bytes()).await.unwrap();
}
