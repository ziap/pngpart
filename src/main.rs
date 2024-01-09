use std::cmp::Ordering;
use std::io::BufWriter;
use std::{env, collections::BinaryHeap};
use std::fs::File;

// TODO: replace with clap for more options
//  - Oxipng settings (enabled, level)
//  - Tolerance/iterations
//  - Verbose (logging, timing)
//  - Glob support
fn get_arguments() -> Option<(Box<str>, Box<str>)> {
    let mut args = env::args();
    let name = args.next()?;

    let in_file = match args.next() {
        Some(in_file) => in_file,
        None => {
            eprintln!("ERROR: no input file");
            eprintln!("USAGE: {name} <input file> <output file>");
            return None;
        }
    };

    match args.next() {
        Some(out_file) => Some((in_file.into(), out_file.into())),
        None => {
            eprintln!("ERROR: no output file");
            eprintln!("USAGE: {name} <input file> <output file>");
            None
        }
    }
}

struct Image {
    width: usize,
    height: usize,

    data: Box<[u8]>
}

fn read_image(path: &str) -> Option<Image> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(err) => {
            eprintln!("ERROR: Failed to open `{path}`: {err}");
            return None;
        }
    };

    let mut decoder = png::Decoder::new(file);
    decoder.set_transformations(png::Transformations::ALPHA);

    let mut reader = match decoder.read_info() {
        Ok(reader) => reader,
        Err(err) => {
            eprintln!("ERROR: Failed to decode `{path}`: {err}");
            return None;
        }
    };

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
            eprintln!("ERROR: Failed to decode `{path}`: {err}");
            return None
        }
    }
}

fn save_image(img: Image, path: &str) -> bool {
    let w = img.width as u32;
    let h = img.height as u32;
    let buf = &img.data as &[u8];

    let mut out_buf = Vec::<u8>::new();

    {
        let mut encoder = png::Encoder::new(BufWriter::new(&mut out_buf), w, h);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);

        let mut writer = match encoder.write_header() {
            Ok(writer) => writer,
            Err(err) => {
                eprintln!("ERROR: Failed to generate PNG header: {err}");
                return false;
            }
        };

        if let Err(err) = writer.write_image_data(buf) {
            eprintln!("ERROR: Failed to encode image to PNG: {err}");
            return false;
        }
    }

    let optimized = match oxipng::optimize_from_memory(&out_buf, &oxipng::Options::default()) {
        Ok(optimized) => optimized,
        Err(err) => {
            eprintln!("ERROR: Failed to optimize image `{path}`: {err}");
            return false;
        }
    };

    match std::fs::write(path, optimized) {
        Ok(()) => true,
        Err(err) => {
            eprintln!("ERROR: Failed to write image to `{path}`: {err}");
            false
        }
    }
}

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

struct HeapItem {
    var: u64,

    bound: Bound
}

impl PartialEq for HeapItem {
    fn eq(&self, other: &Self) -> bool {
        self.var == other.var
    }
}

impl PartialOrd for HeapItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.var.cmp(&other.var))
    }
}

impl Ord for HeapItem {
    fn cmp(&self, other: &Self) -> Ordering {
        self.var.cmp(&other.var)
    }
}

impl Eq for HeapItem {}

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

fn main() {
    let (in_file, out_file) = match get_arguments() {
        Some(filepaths) => filepaths,
        None => std::process::exit(1),
    };

    let img = match read_image(&in_file) {
        Some(img) => img,
        None => std::process::exit(1),
    };

    let mut compressor = Compressor::new(img);
    compressor.compress(128);
    eprintln!("Iterations: {}", compressor.heap.len());
    let out_img = compressor.reconstruct();

    if !save_image(out_img, &out_file) {
        std::process::exit(1) 
    }
}
