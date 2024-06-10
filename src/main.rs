use core::iter::zip;
use dmzj::Api;
use mktemp::Temp;
use reqwest::Url;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::sleep;
use tokio_retry::strategy::{jitter, FixedInterval};
use tokio_retry::Retry;
use trauma::download::{Download, Status};
use trauma::downloader::DownloaderBuilder;

use clap::Parser;

#[derive(Parser, Debug)]
struct Cli {
    #[arg(required = true, help = "the id of the manga")]
    id: u32,

    #[arg(
        short = 'o',
        long = "output",
        help = "output location of the PDF manga"
    )]
    output: Option<PathBuf>,

    #[arg(
        short = 'j',
        long = "jobs",
        help = "number of concurrent downloads",
        default_value_t = 6
    )]
    jobs: usize,

    #[arg(
        short = 'r',
        long = "retries",
        help = "number of retries per HTTP request",
        default_value_t = 5
    )]
    retries: usize,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Cli::parse();

    // let args: Vec<String> = std::env::args().collect();
    // let xdg_dirs = xdg::BaseDirectories::with_prefix("dmzj2pdf").unwrap();
    let api = Api::new();

    let target_id: u32 = args.id;
    println!("manga id = {:?}", target_id);

    let mut toc_creator = TOCCreator::new();

    let response = api.fetch_manga_details(target_id).await.unwrap();
    let detail = &response.data;
    let description = &response.data.description;
    let title = &response.data.title;
    let cover = &response.data.cover;
    let authors: Vec<String> = response
        .data
        .authors
        .iter()
        .map(|au| au.tagName.clone())
        .collect();

    println!("title = {}", title);
    println!("description = {}", description);
    println!("cover = {}", cover);
    println!("authors = {:?}", authors);

    toc_creator.add_title(title);
    toc_creator.add_authors(&authors);

    let mut chapter_pdfs: Vec<PathBuf> = vec![];

    // // Create a reqwest client with a 5-second timeout
    // let client = Client::builder()
    //     .timeout(std::time::Duration::from_secs(15))
    //     .build()?;

    // let cache_dir = xdg_dirs.get_cache_home();

    let temp_merge_dir = Temp::new_dir().expect("Failed to create temporary file for merging");
    let temp_merge_path = temp_merge_dir.to_path_buf(); // xdg_dirs.create_cache_directory("merge").unwrap();
    let retry_strategy = FixedInterval::new(Duration::from_secs(10))
        .map(jitter) // add jitter to delays
        .take(args.retries); // limit to 3 retries

    for chapter in &detail.chapters {
        println!("Chapter section: {:?}", chapter.title);
        let chapters = chapter.data.iter().rev();

        let mut ch = -1;
        let mut page = 1;
        for chapter in chapters {
            ch += 1;

            // if ch >= 1 {
            //     break;
            // }

            println!("Chapter name: {:?}, ch: {:?}", chapter.chapterTitle, ch);
            toc_creator.add_bookmark(page, &chapter.chapterTitle, 1);

            let _target_id = target_id.clone();
            let _chapter_id = chapter.chapterId.clone() as i32;

            let images = Retry::spawn(retry_strategy.clone(), || {
                api.fetch_chapter_images(target_id, chapter.chapterId as i32)
            })
            .await?
            .data;

            page += images.pageUrlHD.len() as i32;

            let temp_dir = Temp::new_dir().expect("Failed to create temporary directory");
            println!("Temporary directory created at: {:?}", temp_dir.as_path());

            let file_paths: Vec<PathBuf> = images
                .pageUrlHD
                .iter()
                .enumerate()
                .map(|(index, url)| url_to_download_path(url, index, temp_dir.as_path()))
                .collect();

            let downloads: Vec<Download> = zip(images.pageUrlHD.clone(), file_paths.clone())
                .map(|(url, path)| Download {
                    url: Url::parse(&url).unwrap(),
                    filename: path.file_name().unwrap().to_str().unwrap().to_string(),
                })
                .collect();

            let downloader = DownloaderBuilder::new()
                .retries(args.retries as u32)
                .concurrent_downloads(args.jobs)
                .directory(temp_dir.to_path_buf())
                .build();

            let summary = downloader.download(&downloads).await;
            let fails: Vec<(String, String)> = summary
                .iter()
                .filter_map(|s| match s.status() {
                    Status::Fail(msg) => Some((s.download().filename.clone(), msg.clone())),
                    _ => None,
                })
                .collect();

            if !fails.is_empty() {
                eprintln!("The following files failed to download:");
                for (f, r) in fails {
                    eprintln!("file: {}, reason: {}", f, r);
                }
                break;
            }

            // let download_tasks = images.pageUrlHD.iter().enumerate().map(|(index, url)| {
            //     download_file(&client, url, &temp_dir, &chapter.chapterTitle, index + 1)
            // });

            // let file_paths: Vec<PathBuf> = join_all(download_tasks)
            //     .await
            //     .into_iter()
            //     .collect::<Result<_, _>>()?;

            let output_pdf_name = format!("{}-{}.pdf", title, chapter.chapterTitle /* ch */);
            let output_pdf_path = temp_merge_path.join(output_pdf_name);
            run_img2pdf(&file_paths, &output_pdf_path).await?;
            chapter_pdfs.push(output_pdf_path);

            sleep(Duration::from_secs(1)).await;
        }

        break;
    }

    let temp_merge_file = /* Path::new("merge.pdf"); // */ temp_merge_path.join("merge.pdf");
    merge_pdfs(&chapter_pdfs, &temp_merge_file).await?;

    let temp_bookmark_file = /* Path::new("toc.txt"); // */ temp_merge_path.join("toc.txt");
    toc_creator.write_bookmark(&temp_bookmark_file).await?;

    let final_output = &args
        .output
        .unwrap_or(PathBuf::from(format!("{}.pdf", title)));
    add_toc(&temp_merge_file, &temp_bookmark_file, &final_output).await?;

    Ok(())
}

async fn add_toc(input: &Path, toc: &Path, out: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut command = Command::new("pdftk");
    command
        .arg(input)
        .arg("update_info_utf8")
        .arg(toc)
        .arg("output")
        .arg(out);

    // Run the command
    println!("Adding TOC to PDF...");
    let output = command.output().await?;

    if !output.status.success() {
        let error_msg = String::from_utf8_lossy(&output.stderr).to_string();
        eprintln!("Error running pdftk to add toc: {}", error_msg);
        return Err(error_msg.into());
    } else {
        println!("PDF merged successfully: {:?}", out);
    }

    Ok(())
}

async fn merge_pdfs(
    pdfs: &Vec<PathBuf>,
    output_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut command = Command::new("pdftk");
    command.args(pdfs).arg("cat").arg("output").arg(output_path);

    // Run the command
    println!("Merging PDF files...");
    let output = command.output().await?;

    if !output.status.success() {
        let error_msg = String::from_utf8_lossy(&output.stderr).to_string();
        eprintln!("Error running pdftk to merge chapters: {}", error_msg);
        return Err(error_msg.into());
    } else {
        println!("PDF merged successfully: {:?}", output_path);
    }

    Ok(())
}

fn url_to_download_path(url: &str, index: usize, temp_dir: &Path) -> PathBuf {
    // Extract the file extension from the URL
    let extension = url.split('.').last().unwrap_or("");

    // Construct the file name
    // let file_name = format!("{}-{}.{}", predefined_string, index, extension);
    let file_name = format!("{}.{}", index, extension);

    temp_dir.join(file_name)
}

// fn should_retry(err: &reqwest::Error) -> bool {
//     // Define the conditions under which a retry should occur
//     true // err.is_timeout() || err.is_connect() || err.is_request()
// }
//
// async fn download_file(
//     client: &Client,
//     url: &str,
//     temp_dir: &Path,
//     _: &str,
//     index: usize,
// ) -> Result<PathBuf, Error> {
//     // Define a retry strategy with a fixed interval of 30 seconds and a maximum of 4 retries
//     let retry_strategy = FixedInterval::from_millis(30000).take(4);
//
//     // Retry the download operation
//     let result = RetryIf::spawn(
//         retry_strategy,
//         || async {
//             // Send GET request
//             println!("Fetching {}", url);
//             client.get(url).send().await
//         },
//         should_retry,
//     )
//     .await;
//
//     let response = result?;
//
//     let file_path = url_to_download_path(url, index, temp_dir);
//
//     // Create and write to the file
//     let mut file: File = File::create(&file_path).await.unwrap();
//     let content = response.bytes().await?;
//     file.write_all(&content).await.unwrap();
//
//     println!("Downloaded {} to {:?}", url, file_path);
//
//     sleep(Duration::from_secs(1));
//
//     Ok(file_path)
// }

async fn run_img2pdf(files: &[PathBuf], output: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let output_path = Path::new(output);

    // Convert PathBufs to Strings for the command
    let file_strings: Vec<String> = files
        .iter()
        .map(|path| path.to_str().unwrap().to_string())
        .collect();

    // Create the img2pdf command
    let mut command = Command::new("img2pdf");
    command
        .arg("--rotation=ifvalid")
        .args(&file_strings)
        .arg("-o")
        .arg(output_path);

    // Run the command
    println!("Converting images to PDF...");
    let output = command.output().await?;

    if !output.status.success() {
        let error_msg = String::from_utf8_lossy(&output.stderr).to_string();
        eprintln!("Error running img2pdf: {}", error_msg);
        Err(error_msg.into())
    } else {
        println!(
            "PDF created successfully: {}",
            output_path.to_str().unwrap()
        );
        Ok(())
    }
}

struct TOCCreator {
    entries: Vec<String>,
}

impl TOCCreator {
    fn new() -> Self {
        TOCCreator {
            entries: Vec::new(),
        }
    }

    fn add_title(&mut self, title: &str) {
        self.add_metadata(&"Title".to_string(), &title.to_string())
    }

    fn add_authors(&mut self, authors: &Vec<String>) {
        self.add_metadata(&"Author".to_string(), &authors.join(","));
    }

    fn add_metadata(&mut self, key: &String, val: &String) {
        let entry = format!(
            r#"
InfoBegin
InfoKey: {}
InfoValue: {}
"#,
            key, val
        );
        self.entries.push(entry);
    }

    fn add_bookmark(&mut self, page_number: i32, title: &str, level: i32) {
        // let mut entry = String::new();
        // writeln!(entry, "BookmarkBegin").unwrap();
        // writeln!(entry, "BookmarkTitle: {}", title).unwrap();
        // writeln!(entry, "BookmarkLevel: {}", level).unwrap();
        // writeln!(entry, "BookmarkPageNumber: {}", page_number).unwrap();
        let entry = format!(
            r#"
BookmarkBegin
BookmarkTitle: {}
BookmarkLevel: {}
BookmarkPageNumber: {}
"#,
            title, level, page_number
        );
        self.entries.push(entry);
    }

    async fn write_bookmark(&self, file_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let mut file = File::create(file_path).await?;
        for entry in &self.entries {
            file.write_all(entry.as_bytes()).await?;
        }
        Ok(())
    }
}
