# BesinVeri
Türkiye'deki gıdaların kalori, makro, mineral ve vitamin değerlerini sunan açık ve ücretsiz bir REST API. 

#### Önemli Not: Bu API şu anda erken test aşamasındadır (v0.1.0). Veritabanında yalnızca test amaçlı sınırlı sayıda (4-5 adet) besin bulunmaktadır. Servis kararlılığı ve veri doğruluğu geliştirme süreci boyunca değişiklik gösterebilir.

## Örnek Kullanım
```Python
import requests

# "Muz" araması yap ve ilk sonucu al:
muz = requests.get("https://api.besinveri.com/foods/search?q=muz").json()[0]
print(f"{muz['description']} (100gr): {muz['energy']} kcal, {muz['potassium']} mg Potasyum")
# Çıktı: Muz (100gr): 89 kcal, 358 mg Potasyum

# "Turuncu" etiketli besinlerin listesini al:
turuncular = requests.get("https://api.besinveri.com/foods/search?q=turuncu&mode=tag").json()
print([f['description'] for f in turuncular])
# Çıktı: ['Portakal', 'Havuç']
```

## Bağlantı Noktaları (Endpoints)

| Açıklama | Metot | Endpoint / URL |
| :--- | :---: | :--- |
| **Sistem Durumu** | `GET` | `https://api.besinveri.com/health` |
| **Besin Detayı** | `GET` | `https://api.besinveri.com/food/{slug}` |
| **Tüm Liste** | `GET` | `https://api.besinveri.com/foods/list` |
| **Arama** | `GET` | `https://api.besinveri.com/foods/search?q={query}&mode={mode}&limit={limit}` |
| **Etiketler** | `GET` | `https://api.besinveri.com/tags` |

## Kaynak Verimliliği
Proje mimarisinde Rust ve Axum tercih edilerek, minimum donanım kaynağı (memory footprint) ile yüksek ölçeklenebilirlik hedeflenmiştir. Konteynerize ortamda (Docker) yapılan testlerde, API servisi 'idle' durumdayken ortalama 1.5 MiB RAM tüketimi ile çalışmaktadır. Bu, geleneksel backend teknolojilerine kıyasla sistem yükünü minimize eder.

<img width="1505" alt="image" src="https://karahanbuhan.com/i/5ca23bde657b48ef832d47c7403b9dd7.png" />


## Gereksinimler
- [Git](https://git-scm.com/downloads)
- [Rust Programlama Dili ve Cargo](https://www.rust-lang.org/tools/install)

## Kurulum
1. Öncelikle kaynak kodunu bilgisayarınıza indirin: `git clone https://github.com/karahanbuhan/besinveri.git`
2. Daha sonrasında kaynak kodunun olduğu ana klasöre girin: `cd besinveri`
3. En hızlı şekilde çalıştırmak için `cargo run` komutunu kullanabilirsiniz.

## Docker ile Kurulum
1. Öncelikle kaynak kodunu bilgisayarınıza indirin: `git clone https://github.com/karahanbuhan/besinveri.git`
2. Daha sonrasında kaynak kodunun olduğu ana klasöre girin: `cd besinveri`
3. Docker Image oluşturmak ve bunu konteyner olarak çalıştırmak için: `docker build --tag besinveri . && docker run -p 8099:8099 --detach besinveri`

## Kurulum Sonrası Kılavuz
Kurulum yaptıktan ve BesinVeri'yi çalıştırdıktan sonra, :8099/TCP portundan API ve siteye bağlanabilirsiniz. Eğer localhost üzerinden açtıysanız, http://localhost:8099/ adresine girerek kurulumun başarılı olduğuna emin olun. Eğer hata alıyorsanız, kurulumu doğru yaptığınıza ve BesinVeri'nin açık olduğuna emin olun.

***Dikkat:** Docker ile çalıştırmak, `cargo run --release` kullanıldığı ve Image inşaa edildiği için birkaç dakika sürebilir. Bu sebepten dolayı, sadece arayüzün son kullanıcıya sunulduğu ortamda bu şekilde çalıştırılması önerilir.*

