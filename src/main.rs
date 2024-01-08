use std::io::BufWriter;
use std::{env, collections::BinaryHeap};
use std::fs::File;
use std::time::Instant;

// TODO: replace with clap for more options
//  - Oxipng settings (enabled, level)
//  - Tolerance/iterations
//  - Verbose (logging, timing)
//  - Glob support
fn get_arguments() -> Option<(Box<str>, Box<str>)> {
    let mut args = env::args();
    let name = args.next()?;
    if let Some(in_file) = args.next() {
        if let Some(out_file) = args.next() {
            Some((in_file.into(), out_file.into()))
        } else {
            eprintln!("ERROR: no output file");
            eprintln!("USAGE: {name} <input file> <output file>");
            None
        }
    } else {
        eprintln!("ERROR: no input file");
        eprintln!("USAGE: {name} <input file> <output file>");
        None
    }
}

struct Image {
    width: usize,
    height: usize,

    data: Box<[u8]>
}

fn read_image(path: &str) -> Option<Image> {
    match File::open(path) {
        Ok(file) => {
            let mut decoder = png::Decoder::new(file);
            decoder.set_transformations(png::Transformations::ALPHA);
            let mut reader = decoder.read_info().unwrap();
            let mut buf = vec![0u8; reader.output_buffer_size()];
            match reader.next_frame(&mut buf) {
                Ok(info) => {
                    buf.resize(info.buffer_size(), 0);
                    Some(Image {
                        width: info.width as usize,
                        height: info.height as usize,
                        data: buf.into_boxed_slice()
                    })
                },
                Err(err) => {
                    eprintln!("ERROR: Problem decoding image `{path}`: {err}");
                    None
                }
            }
        },
        Err(err) => {
            eprintln!("ERROR: Problem opening image `{path}`: {err}");
            None
        }
    }
}

fn save_image(img: Image, path: &str) {
    let w = img.width as u32;
    let h = img.height as u32;
    let buf = &img.data as &[u8];

    let mut out_buf = Vec::<u8>::new();
    
    {
        let mut encoder = png::Encoder::new(BufWriter::new(&mut out_buf), w, h);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);

        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(buf).unwrap();
    }

    match oxipng::optimize_from_memory(&out_buf, &oxipng::Options::default()) {
        Ok(optimized) => {
            if let Err(err) = std::fs::write(path, optimized) {
                eprintln!("ERROR: Failed to write image `{path}`: {err}")
            };
        },
        Err(err) => eprintln!("ERROR: Failed to optimize image `{path}`: {err}")
    }
}

// TODO: Remove all traits
#[derive(Eq, Ord, PartialEq, PartialOrd)]
struct Bound {
    x_min: usize,
    x_max: usize,
    y_min: usize,
    y_max: usize
}

impl Bound {
    fn new(x_min: usize, x_max: usize, y_min: usize, y_max: usize) -> Self {
        Self { x_min, x_max, y_min, y_max }
    }
}

fn compute_mean(img: &Image, bound: &Bound) -> [u64; 4] {
    let mut mean = [0u64; 4];
    for i in bound.y_min..bound.y_max {
        for j in bound.x_min..bound.x_max {
            for k in 0..4 {
                mean[k] += img.data[4 * (i * img.width + j) + k] as u64;
            }
        }
    }

    for elem in mean.iter_mut() {
        let w = (bound.x_max - bound.x_min) as u64;
        let h = (bound.y_max - bound.y_min) as u64;
        *elem /= w * h;
    }

    mean
}

// TODO: Reimplement these traits
#[derive(Eq, Ord, PartialEq, PartialOrd)]
struct HeapItem {
    var: u64,
    
    bound: Bound
}

impl HeapItem {
    fn new(img: &Image, bound: Bound) -> Self {
        let mean = compute_mean(img, &bound);

        let mut variance: u64 = 0;
        for i in bound.y_min..bound.y_max {
            for j in bound.x_min..bound.x_max {
                for k in 0..4 {
                    let diff = img.data[4 * (i * img.width + j) + k] as i64 - mean[k] as i64;
                    variance += (diff * diff) as u64;
                }
            }
        }
        Self { var: variance, bound }
    }
}

struct Compressor {
    img: Image,
    heap: BinaryHeap<HeapItem>,
}

impl Compressor {
    fn new(img: Image) -> Self {
        let mut heap = BinaryHeap::<HeapItem>::new();
        heap.push(HeapItem::new(&img, Bound::new(0, img.width, 0, img.height)));
        Self { img, heap }
    }

    fn compress(&mut self, tolerance: u64) {
        while self.heap.peek().unwrap().var > tolerance {
            self.add_detail();
        }
    }

    fn add_detail(&mut self) {
        let item = self.heap.pop().unwrap();
        let bound = item.bound;

        let split_x = (bound.x_max + bound.x_min) / 2;
        let split_y = (bound.y_max + bound.y_min) / 2;

        let bx0 = Bound::new(bound.x_min, split_x, bound.y_min, bound.y_max);
        let bx1 = Bound::new(split_x, bound.x_max, bound.y_min, bound.y_max);
        let by0 = Bound::new(bound.x_min, bound.x_max, bound.y_min, split_y);
        let by1 = Bound::new(bound.x_min, bound.x_max, split_y, bound.y_max);

        if split_x > bound.x_min && bound.x_max > split_x {
            let ix0 = HeapItem::new(&self.img, bx0);
            let ix1 = HeapItem::new(&self.img, bx1);

            if split_y > bound.y_min && bound.y_max > split_y {
                let iy0 = HeapItem::new(&self.img, by0);
                let iy1 = HeapItem::new(&self.img, by1);

                if ix0.var + ix1.var < iy0.var + iy1.var {
                    self.heap.push(ix0);
                    self.heap.push(ix1);
                } else {
                    self.heap.push(iy0);
                    self.heap.push(iy1);
                }
            } else {
                self.heap.push(ix0);
                self.heap.push(ix1);
            }
        } else {
            self.heap.push(HeapItem::new(&self.img, by0));
            self.heap.push(HeapItem::new(&self.img, by1));
        }
    }

    fn reconstruct(self) -> Image {
        let mut out_img = Image {
            width: self.img.width,
            height: self.img.height,
            
            data: vec![0u8; self.img.data.len()].into_boxed_slice()
        };

        for item in self.heap {
            let mean = compute_mean(&self.img, &item.bound);

            for i in item.bound.y_min..item.bound.y_max {
                for j in item.bound.x_min..item.bound.x_max {
                    let idx = 4 * (i * out_img.width + j);
                    for k in 0..4 {
                        out_img.data[idx + k] = mean[k] as u8;
                    }
                }
            }
        }

        out_img
    }
}

fn process_image(img: Image) -> Image {
    let mut compressor = Compressor::new(img);
    compressor.compress(128);
    compressor.reconstruct()
}

fn main() {
    if let Some((in_file, out_file)) = get_arguments() {
        if let Some(img) = read_image(&in_file) {
            eprint!("INFO: Compressing `{in_file}`... ");
            let compress_start = Instant::now();
            let out_img = process_image(img);
            eprintln!("DONE [{}ms]", compress_start.elapsed().as_millis());
            eprint!("INFO: Optimizing `{in_file}`... ");
            let optimize_start = Instant::now();
            save_image(out_img, &out_file);
            eprintln!("DONE [{}ms]", optimize_start.elapsed().as_millis());
        };
    }
}
