import sharp from 'sharp';
import { writeFileSync } from 'fs';
import toIco from 'to-ico';

const sizes = [16, 24, 32, 48, 64, 128, 256];
const source = 'src-tauri/icons/icon.png';

async function main() {
    const buffers = await Promise.all(
        sizes.map(size =>
            sharp(source)
                .resize(size, size)
                .png()
                .toBuffer()
        )
    );

    const ico = await toIco(buffers);
    writeFileSync('src-tauri/icons/icon.ico', ico);
    console.log('icon.ico created successfully!');
}

main().catch(console.error);
