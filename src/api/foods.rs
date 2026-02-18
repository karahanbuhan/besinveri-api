use std::{collections::BTreeMap, net::SocketAddr};

use axum::{
    Json,
    extract::{ConnectInfo, Path, Query, State},
    http::{HeaderMap, StatusCode},
};

use anyhow::Result;
use serde::Deserialize;
use tracing::{debug, error};

use crate::{
    SharedState,
    api::{database, error::APIError, parse_client_ip},
    core::food::Food,
};

pub(crate) async fn food(
    Path(slug): Path<String>,
    State(shared_state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Json<Food>, APIError> {
    // Girilen yemek isminin, istediğimiz limitler içinde olduğuna emin olalım, DoS'a karşı karakter limiti ekleyelim.
    if slug.is_empty() || slug.len() > 100 {
        return Err(APIError::new(
            StatusCode::BAD_REQUEST,
            "Slug en az 1 karakter, en fazla 100 karakterden oluşabilir",
        ));
    }

    sanitize_input(&slug)?;

    let mut food = database::select_food_by_slug(&*shared_state.api_db.lock().await, &slug)
        .await
        .map_err(|e| {
            error!("Veritabanı yemek bilgisi sorgularken hata oluştu: {:?}", e);
            APIError::new(
                StatusCode::NOT_FOUND,
                "Bu yemekle ilgili veriye ulaşılamadı",
            )
        })?;

    fix_image_url(&State(shared_state), &mut food).await;

    if food.verified.is_some_and(|verified| verified) {
        debug!(
            "GET /food: ({}), {}",
            slug,
            parse_client_ip(&addr, &headers)
        );
        Ok(Json(food))
    } else {
        Err(APIError::new(
            StatusCode::FORBIDDEN,
            "Bu yemek henüz onaylanmadığı için gösterilemiyor",
        ))
    }
}

pub(crate) async fn foods(
    State(shared_state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Json<BTreeMap<&'static str, String>> {
    // Henüz test etmedim ama ne olur ne olmaz diye to_owned atıyorum birkaç ms olsa bile config'e blok atılmaması için
    let api_base_url = &shared_state.config.lock().await.api.base_url.to_owned();
    let mut endpoints: BTreeMap<&'static str, String> = BTreeMap::new();

    endpoints.insert(
        "list_all_foods_url",
        format!("{}/{}", &api_base_url, "foods/list"),
    );
    endpoints.insert(
        "search_food_url",
        format!(
            "{}/{}",
            api_base_url, "foods/search?q={query}&mode={description, tag}&limit={limit}"
        ),
    );

    debug!(
        "GET /foods: ({} bağlantı noktası), {}",
        endpoints.len(),
        parse_client_ip(&addr, &headers)
    );
    Json(endpoints)
}

// HashMap yerine BTreeMap kullanma sebebimiz, yemek isimlerini alfabetik sıralamak istememiz. HashMap kullansaydık her seferinde rastgele sıralama olacaktı
pub(crate) async fn foods_list(
    State(shared_state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Json<BTreeMap<String, String>>, APIError> {
    let slugs = database::select_all_foods_slugs(&*shared_state.api_db.lock().await)
        .await
        .map_err(|e| {
            error!(
                "Veritabanı yemek açıklamaları sorgularken hata oluştu: {:?}",
                e
            );
            APIError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Veritabanı yemek sorgusu yapılırken hata oluştu",
            )
        })?;

    let api_base_url = &shared_state.config.lock().await.api.base_url;

    debug!(
        "GET /foods/list: ({} yemek), {}",
        slugs.len(),
        parse_client_ip(&addr, &headers)
    );
    Ok(Json(
        slugs
            .into_iter()
            .map(|slug| slug)
            // Daha sonra fuji-elma: https://API_BASE.URL/food/food1\n.../food2 şeklinde gösteriyoruz
            .map(|slug| (slug.clone(), api_base_url.clone() + "/food/" + &slug))
            .collect(),
    ))
}

pub(crate) async fn tags_list(
    State(shared_state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Json<Vec<String>>, APIError> {
    let tags = database::select_all_tags(&*shared_state.api_db.lock().await)
        .await
        .map_err(|e| {
            error!(
                "Veritabanı etiket açıklamaları sorgularken hata oluştu: {:?}",
                e
            );
            APIError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Veritabanı etiket sorgusu yapılırken hata oluştu",
            )
        })?;

    debug!(
        "GET /tags: ({} etiket), {}",
        tags.len(),
        parse_client_ip(&addr, &headers)
    );
    Ok(Json(tags))
}

#[derive(Deserialize)]
pub(crate) struct SearchParams {
    // Sorgu değeri: q
    q: String,
    mode: Option<String>,
    limit: Option<u64>,
}

impl SearchParams {
    fn size(self: &SearchParams) -> usize {
        let query_size = self.q.len();
        let mode_size = self.mode.as_ref().map_or(0, |m| m.len());
        // SearchParams'ın statik boyutunu da ekliyoruz
        size_of::<SearchParams>() + query_size + mode_size
    }
}

pub(crate) async fn foods_search(
    params: Query<SearchParams>,
    State(shared_state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Json<Vec<Food>>, APIError> {
    // Parametrelerin boyutunun 96 baytı geçmesini beklemiyoruz, DoS tarzı saldırıları önlemek için böyle bir önlem alıyoruz
    if params.size() > 96 {
        return Err(APIError::new(
            StatusCode::BAD_REQUEST,
            "Gönderdiğiniz sorgu 96 bayt limitini aşıyor!",
        ));
    }

    // Moda göre uygun veritabanı sorgusunu atıyoruz
    let mode = match &params.mode {
        Some(mode) => mode.to_lowercase(),
        None => "description".to_owned(),
    };

    // Eğer limit girilmemişse ilk 5 sonucu varsayılan olarak döndüreceğiz çünkü arama menülerinde genellikle bu şekilde kullanılıyor
    // Bu limiti daha sonra ekleyeceğiz, sort yapmadan önce eklersek asıl göstermemiz gereken en alakalı yemekleri gösteremeyebiliriz
    let limit = params.limit.unwrap_or(5);
    if limit > shared_state.config.lock().await.api.search_max_limit {
        return Err(APIError::new(
            StatusCode::BAD_REQUEST,
            "Arama limitini geçtiniz!",
        ));
    }

    sanitize_input(&params.q)?;

    let mut foods = match mode.as_str() {
        // İsim ile aratmada ayrıca sıralıyoruz benzerliğine göre
        "description" | "name" => {
            let db = &*shared_state.api_db.lock().await;
            let mut foods = database::search_foods_by_description_wild(db, &params.q)
                .await
                .map_err(|_| {
                    APIError::new(
                        StatusCode::NOT_FOUND,
                        "Veritabanına yemek sorgusu atılırken bir hata oluştu",
                    )
                })?;

            // Yemeklerin alakasına göre sıralıyoruz, örneğin query=Elm için 1. Elma, 2. Fuji Elma ... gibi
            sort_foods_by_query(&mut foods, &params.q).await;

            Ok(foods)
        }

        "tag" => {
            let db = &*shared_state.api_db.lock().await;
            let foods = database::search_foods_by_tag_wild(db, &params.q)
                .await
                .map_err(|_| {
                    APIError::new(
                        StatusCode::NOT_FOUND,
                        "Etiket ile yemek ararken sonuç bulunamadı",
                    )
                })?;

            Ok(foods)
        }

        _ => Err(APIError::new(StatusCode::BAD_REQUEST, "Geçersiz sorgu!")),
    }?;

    // Onaylanmamış yemekleri döndürmüyoruz
    foods.retain(|food| food.verified.unwrap_or(false));
    // Sadece limit kadar yemeğe ihtiyacımız var, gerisini siliyoruz
    foods.truncate(limit as usize);
    // Kalan yemeklerin de resim URL'lerini düzeltiyoruz
    fix_image_urls(&State(shared_state), &mut foods).await;

    debug!(
        "GET /foods/search: mod={}, limit={}, sorgu=\"{}\", ({} yemek), {}",
        mode.as_str(),
        limit,
        &params.q,
        foods.len(),
        parse_client_ip(&addr, &headers)
    );
    Ok(Json(foods))
}

fn sanitize_input(s: &str) -> Result<(), APIError> {
    // Normal bir yemek isminde olmaması gereken karakterler var mı diye de bakalım.
    // Bu karakterler kullanılsa dahi sorun olmaması lazım, yine de önlemimizi alalım.
    if s.contains("..")
        || s.contains("/")
        || s.contains("\\")
        || s.contains("\0")
        || s.contains(";")
        || s.contains("*")
        || s.contains("--")
        || s.contains("/*")
        || s.contains("*/")
        || s.contains("'")
        || s.contains("\"")
        || s.contains("\\")
        || s.trim().is_empty()
    {
        return Err(APIError::new(
            StatusCode::BAD_REQUEST,
            "Sorgu geçersiz karakterler içeriyor",
        ));
    }

    Ok(())
}

async fn fix_image_urls(State(shared_state): &State<SharedState>, foods: &mut Vec<Food>) {
    // Eğer bir yemeğin resim URL'si / ile başlıyorsa, örneğin /images/muz.webp gibi, https://api.besinveri.com/images/muz.webp formatına getirilmeli
    let base_url = &shared_state.config.lock().await.api.static_url;
    foods
        .iter_mut()
        .filter(|food| food.image_url.starts_with("/"))
        .for_each(|food| food.image_url = format!("{}{}", base_url, food.image_url));
}

async fn fix_image_url(State(shared_state): &State<SharedState>, food: &mut Food) {
    // Eğer yemeğin resim URL'si / ile başlıyorsa, örneğin /images/muz.webp gibi, https://api.besinveri.com/images/muz.webp formatına getirilmeli
    if food.image_url.starts_with("/") {
        let base_url = &shared_state.config.lock().await.api.static_url;
        food.image_url = format!("{}{}", base_url, food.image_url);
    }
}

async fn sort_foods_by_query(foods: &mut Vec<Food>, query: &str) {
    let query = query.to_lowercase();

    // (original_index, yemek ref, skor)
    let mut scored: Vec<(usize, Food, u64)> = foods
        .drain(..)
        .enumerate()
        .filter_map(|(idx, food)| {
            // Öncelikle sıralarken prefix şeklinde eşleşenlere öncelik vereceğiz
            // Örneğin ka diye aratıldığında 0: K*ar*puz, 1: Porta*ka*l şeklinde sıralamak istiyoruz
            // Bunun için basit bir puanlama sistemi yapıp bu puanlara göre sort edeceğiz, her eşleşen karakter için 1 puan ekleyeceğiz
            let desc_lower = food.description.to_lowercase();
            if desc_lower.starts_with(&query) {
                return Some((idx, food, 20u64));
            }

            // Prefix kontrolünü hiç geçemeyen yemekler için, örneğin ka diye arattığımızda Porta*ka*l ve Ma*ka*rna makarnanın öncelikli olmasını istiyoruz
            // Başa ne kadar yakınsa o kadar yüksek puan olacak yani, pozisyona göre puan vereceğiz
            if let Some(pos) = desc_lower.find(&query) {
                let len = desc_lower.len();
                let score = 10 * (len.saturating_sub(pos)) / len.max(1);
                return Some((idx, food, score as u64));
            }

            // Eğer hiçbir kontrole uymuyorsa buraya gelmiş olması mantıksız (SQL LIKE'da bir sorun yoksa), en kötü ihtimalle find'da bulunması gerek, yine de düşük bir skorla döndürelim.
            return Some((idx, food, 0 as u64));
        })
        .collect();

    // Puanlara göre yüksekten düşüğe sıralıyoruz
    scored.sort_unstable_by(|a, b| b.2.cmp(&a.2));

    // Sıralanmış yemekleri de birleştirip güncelliyoruz
    *foods = scored.into_iter().map(|(_, food, _)| food).collect();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    // Test verisi oluşturan helper fonksiyonlar
    fn create_test_foods() -> Vec<Food> {
        let mut servings = BTreeMap::new();
        servings.insert("portion".to_string(), 100.0);

        vec![
            // Prefix match: "kar" ile başlar
            Food {
                id: Some(1),
                slug: Some("karpuz".to_string()),
                description: "Karpuz yaz meyvesi olarak bilinir".to_string(),
                verified: Some(true),
                image_url: "http://example.com/karpuz.jpg".to_string(),
                source: "Wikipedia".to_string(),
                tags: vec!["meyve".to_string(), "yaz".to_string()],
                allergens: vec![],
                servings: servings.clone(),
                glycemic_index: 72.0,
                energy: 30.0,
                carbohydrate: 7.55,
                protein: 0.61,
                fat: 0.15,
                saturated_fat: 0.0,
                trans_fat: 0.0,
                sugar: 6.2,
                fiber: 0.4,
                cholesterol: 0.0,
                sodium: 1.0,
                potassium: 112.0,
                water: 91.45,
                iron: 0.24,
                magnesium: 10.0,
                calcium: 16.0,
                zinc: 0.17,
                vitamin_a: 28.0,
                vitamin_b6: 0.045,
                vitamin_b12: 0.0,
                vitamin_c: 7.5,
                vitamin_d: 0.0,
                vitamin_e: 0.05,
                vitamin_k: 0.1,
            },
            // Contains "kar" in middle
            Food {
                id: Some(2),
                slug: Some("portakal".to_string()),
                description: "Portakal C vitamini açısından zengindir".to_string(),
                verified: Some(true),
                image_url: "http://example.com/portakal.jpg".to_string(),
                source: "Wikipedia".to_string(),
                tags: vec!["meyve".to_string(), "narenciye".to_string()],
                allergens: vec![],
                servings: servings.clone(),
                glycemic_index: 43.0,
                energy: 47.0,
                carbohydrate: 11.75,
                protein: 0.94,
                fat: 0.12,
                saturated_fat: 0.0,
                trans_fat: 0.0,
                sugar: 9.35,
                fiber: 2.4,
                cholesterol: 0.0,
                sodium: 0.0,
                potassium: 181.0,
                water: 86.75,
                iron: 0.1,
                magnesium: 10.0,
                calcium: 40.0,
                zinc: 0.07,
                vitamin_a: 11.0,
                vitamin_b6: 0.06,
                vitamin_b12: 0.0,
                vitamin_c: 53.2,
                vitamin_d: 0.0,
                vitamin_e: 0.18,
                vitamin_k: 0.0,
            },
            // Contains "kar" at end
            Food {
                id: Some(3),
                slug: Some("makarna".to_string()),
                description: "Makarna İtalyan mutfağının temelidir".to_string(),
                verified: Some(true),
                image_url: "http://example.com/makarna.jpg".to_string(),
                source: "Wikipedia".to_string(),
                tags: vec!["makarna".to_string(), "italyan".to_string()],
                allergens: vec!["gluten".to_string()],
                servings: servings.clone(),
                glycemic_index: 50.0,
                energy: 371.0,
                carbohydrate: 75.0,
                protein: 13.0,
                fat: 1.5,
                saturated_fat: 0.3,
                trans_fat: 0.0,
                sugar: 2.7,
                fiber: 2.5,
                cholesterol: 0.0,
                sodium: 6.0,
                potassium: 223.0,
                water: 8.8,
                iron: 1.3,
                magnesium: 53.0,
                calcium: 21.0,
                zinc: 1.2,
                vitamin_a: 0.0,
                vitamin_b6: 0.08,
                vitamin_b12: 0.0,
                vitamin_c: 0.0,
                vitamin_d: 0.0,
                vitamin_e: 0.11,
                vitamin_k: 0.0,
            },
            // No match
            Food {
                id: Some(4),
                slug: Some("elma".to_string()),
                description: "Elma güneydoğu Asya kökenlidir".to_string(),
                verified: Some(false),
                image_url: "http://example.com/elma.jpg".to_string(),
                source: "Wikipedia".to_string(),
                tags: vec!["meyve".to_string()],
                allergens: vec![],
                servings: servings.clone(),
                glycemic_index: 39.0,
                energy: 52.0,
                carbohydrate: 13.81,
                protein: 0.26,
                fat: 0.17,
                saturated_fat: 0.0,
                trans_fat: 0.0,
                sugar: 10.39,
                fiber: 2.4,
                cholesterol: 0.0,
                sodium: 1.0,
                potassium: 107.0,
                water: 85.56,
                iron: 0.12,
                magnesium: 5.0,
                calcium: 6.0,
                zinc: 0.04,
                vitamin_a: 3.0,
                vitamin_b6: 0.041,
                vitamin_b12: 0.0,
                vitamin_c: 4.6,
                vitamin_d: 0.0,
                vitamin_e: 0.18,
                vitamin_k: 2.2,
            },
        ]
    }

    fn generate_large_food_dataset(size: usize) -> Vec<Food> {
        let mut foods = Vec::with_capacity(size);
        let mut servings = BTreeMap::new();
        servings.insert("portion".to_string(), 100.0);

        for i in 0..size {
            let description = match i % 5 {
                0 => format!("Karpuz {} yaz meyvesi olarak bilinir", i),
                1 => format!("Portakal {} C vitamini açısından zengindir", i),
                2 => format!("Makarna {} İtalyan mutfağının temelidir", i),
                3 => format!("Elma {} güneydoğu Asya kökenlidir", i),
                _ => format!("Meyve {} farklı türde bulunur", i),
            };

            foods.push(Food {
                id: Some(i as i64),
                slug: Some(format!("food-{}", i)),
                description,
                verified: Some(true),
                image_url: format!("http://example.com/food-{}.jpg", i),
                source: "Test Data".to_string(),
                tags: vec![format!("tag-{}", i % 3)],
                allergens: vec![],
                servings: servings.clone(),
                glycemic_index: 50.0 + (i as f64 % 50.0), // 50-100 arası rastgele
                energy: 100.0 + (i as f64 % 400.0),       // 100-500 arası
                carbohydrate: 20.0 + (i as f64 % 60.0),   // 20-80 arası
                protein: 5.0 + (i as f64 % 20.0),         // 5-25 arası
                fat: 3.0 + (i as f64 % 15.0),             // 3-18 arası
                saturated_fat: 1.0 + (i as f64 % 5.0),    // 1-6 arası
                trans_fat: 0.0,
                sugar: 10.0 + (i as f64 % 30.0), // 10-40 arası
                fiber: 2.0 + (i as f64 % 8.0),   // 2-10 arası
                cholesterol: 0.0,
                sodium: 50.0 + (i as f64 % 100.0), // 50-150 arası
                potassium: 200.0 + (i as f64 % 300.0), // 200-500 arası
                water: 80.0 + (i as f64 % 20.0),   // 80-100 arası
                iron: 1.0 + (i as f64 % 2.0),      // 1-3 arası
                magnesium: 20.0 + (i as f64 % 40.0), // 20-60 arası
                calcium: 30.0 + (i as f64 % 70.0), // 30-100 arası
                zinc: 0.5 + (i as f64 % 1.5),      // 0.5-2 arası
                vitamin_a: 100.0 + (i as f64 % 200.0), // 100-300 arası
                vitamin_b6: 0.1 + (i as f64 % 0.2), // 0.1-0.3 arası
                vitamin_b12: 0.0,
                vitamin_c: 50.0 + (i as f64 % 100.0), // 50-150 arası
                vitamin_d: 5.0 + (i as f64 % 10.0),   // 5-15 arası
                vitamin_e: 2.0 + (i as f64 % 3.0),    // 2-5 arası
                vitamin_k: 10.0 + (i as f64 % 20.0),  // 10-30 arası
            });
        }

        foods
    }

    // Testleri async yap
    #[tokio::test]
    async fn performance_test_small_dataset() {
        let mut foods = generate_large_food_dataset(100);
        let query = "kar";
        let start = Instant::now();
        sort_foods_by_query(&mut foods, query).await; // .await ekle
        let duration = start.elapsed();

        println!("100 foods: {:?}", duration);
        assert!(duration.as_millis() < 10);
        assert_eq!(foods.len(), 100);
    }

    #[tokio::test]
    async fn performance_test_medium_dataset() {
        let mut foods = generate_large_food_dataset(1000);
        let query = "kar";
        let start = Instant::now();
        sort_foods_by_query(&mut foods, query).await;
        let duration = start.elapsed();

        println!("1000 foods: {:?}", duration);
        assert!(duration.as_millis() < 50);
        assert_eq!(foods.len(), 1000);
    }

    #[tokio::test]
    async fn performance_test_large_dataset() {
        let mut foods = generate_large_food_dataset(5000);
        let query = "kar";
        let start = Instant::now();
        sort_foods_by_query(&mut foods, query).await;
        let duration = start.elapsed();

        println!("5000 foods: {:?}", duration);
        assert!(duration.as_millis() < 250);
        assert_eq!(foods.len(), 5000);
    }

    #[tokio::test]
    async fn performance_regression_test() {
        let sizes = [(100usize, 10u64), (1000usize, 50u64), (5000usize, 250u64)];
        let query = "kar";

        for &(size, max_ms) in &sizes {
            println!("\nTesting {} foods (max {}ms)", size, max_ms);

            let mut foods = generate_large_food_dataset(size);
            let start = Instant::now();
            sort_foods_by_query(&mut foods, query).await; // ✅ .await
            let duration = start.elapsed();

            let ms = duration.as_millis();

            println!("Duration: {}ms", ms);

            assert!(
                ms < max_ms as u128,
                "{} foods için {}ms (max: {}ms)",
                size,
                ms,
                max_ms
            );
            assert_eq!(foods.len(), size);
        }
    }

    // Diğer unit testleri de güncelle
    #[tokio::test]
    async fn test_sort_by_query_prefix_match() {
        let mut foods = create_test_foods();
        sort_foods_by_query(&mut foods, "kar").await; // ✅ .await

        assert_eq!(foods[0].slug, Some("karpuz".to_string()));
        assert_eq!(foods[1].slug, Some("makarna".to_string()));
        assert_eq!(foods[2].slug, Some("portakal".to_string()));
        assert_eq!(foods[3].slug, Some("elma".to_string()));
    }

    #[tokio::test]
    async fn test_sort_by_query_position_scoring() {
        let mut foods = vec![
            // "kaşar" başlangıçta
            Food {
                id: Some(1),
                slug: Some("baslangic".to_string()),
                description: "Kaşar peyniri başlangıçta kullanılır".to_string(),
                verified: Some(true),
                image_url: "".to_string(),
                source: "".to_string(),
                tags: vec![],
                allergens: vec![],
                servings: BTreeMap::new(),
                ..Default::default()
            },
            // "kaşar" ortada
            Food {
                id: Some(2),
                slug: Some("orta".to_string()),
                description: "Peynir kaşar peyniri ortada erir".to_string(),
                verified: Some(true),
                image_url: "".to_string(),
                source: "".to_string(),
                tags: vec![],
                allergens: vec![],
                servings: BTreeMap::new(),
                ..Default::default()
            },
            // "kaşar" sonda
            Food {
                id: Some(3),
                slug: Some("son".to_string()),
                description: "Peynir ortada kaşar peyniri sonda".to_string(),
                verified: Some(true),
                image_url: "".to_string(),
                source: "".to_string(),
                tags: vec![],
                allergens: vec![],
                servings: BTreeMap::new(),
                ..Default::default()
            },
        ];

        sort_foods_by_query(&mut foods, "kaşar").await;

        // Başlangıçta olan en yüksek skor almalı (20 puan)
        assert_eq!(foods[0].slug, Some("baslangic".to_string()));
        // Sonra ortada olan (orta pozisyon puanı)
        assert_eq!(foods[1].slug, Some("orta".to_string()));
        // En son sondaki (düşük pozisyon puanı)
        assert_eq!(foods[2].slug, Some("son".to_string()));
    }

    #[tokio::test]
    async fn test_sort_by_query_no_match() {
        let mut foods = create_test_foods();
        let original_order = foods.clone();

        sort_foods_by_query(&mut foods, "xyz").await; // Hiçbir şeyle eşleşmez

        // Sıralama değişmemeli (hepsi 0 skor)
        assert_eq!(foods, original_order);
    }

    #[tokio::test]
    async fn test_sort_by_query_empty_query() {
        let mut foods = create_test_foods();
        let original_order = foods.clone();

        sort_foods_by_query(&mut foods, "").await;

        // Boş query ile sıralama değişmemeli
        assert_eq!(foods, original_order);
    }

    #[tokio::test]
    async fn test_sort_by_query_empty_vec() {
        let mut foods: Vec<Food> = vec![];
        let original = foods.clone();

        sort_foods_by_query(&mut foods, "test").await;

        assert_eq!(foods, original);
    }

    #[tokio::test]
    async fn test_sort_by_query_case_insensitive() {
        let mut foods = create_test_foods();

        sort_foods_by_query(&mut foods, "KaR").await;

        // Büyük/küçük harf duyarlılığı olmamalı
        assert_eq!(foods[0].slug, Some("karpuz".to_string()));
    }

    #[tokio::test]
    async fn test_sort_by_query_stable_sort() {
        // Aynı skora sahip elementlerin orijinal sıralarını koruması için
        let mut foods = vec![
            Food {
                id: Some(1),
                slug: Some("karpuz".to_string()),
                description: "Karpuz".to_string(),
                verified: Some(true),
                image_url: "".to_string(),
                source: "".to_string(),
                tags: vec![],
                allergens: vec![],
                servings: BTreeMap::new(),
                ..Default::default()
            },
            Food {
                id: Some(2),
                slug: Some("portakal".to_string()),
                description: "Portakal".to_string(),
                verified: Some(true),
                image_url: "".to_string(),
                source: "".to_string(),
                tags: vec![],
                allergens: vec![],
                servings: BTreeMap::new(),
                ..Default::default()
            },
        ];

        let original_order = foods.clone();
        sort_foods_by_query(&mut foods, "ka").await;

        // Aynı skorlu elementler orijinal sıralarını korumalı
        assert_eq!(foods, original_order);
    }
}
