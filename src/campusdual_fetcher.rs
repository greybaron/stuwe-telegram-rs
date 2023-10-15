use scraper::{Html, Selector};

async fn extract_data(html_text: String) -> Vec<(String, String, usize)> {
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

        grades.push((name.to_string(), grade.to_string(), sub_count));
    }

    grades
}

pub async fn get_campusdual_grades(
    uname: String,
    password: String,
) -> Result<Vec<(String, String, usize)>, Box<dyn std::error::Error>> {
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .build()?;

    let resp = client
        .get("https://erp.campus-dual.de/sap/bc/webdynpro/sap/zba_initss?sap-client=100&sap-language=de&uri=https://selfservice.campus-dual.de/index/login")
        .send()
        .await?;
    println!("load site: {}", resp.status());

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

    log::debug!("zba_initss: {}", resp.status());

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
            "Anmeldung" => panic!("Login failed"),
            "Initialisierung Selfservices" => {}
            _ => log::warn!("unerwarteter Seitenname nach Anmeldung (behandle wie Erfolg...)"),
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
