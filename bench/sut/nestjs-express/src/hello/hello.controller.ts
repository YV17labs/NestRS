import { Readable } from 'node:stream';
import {
  Body,
  Controller,
  Get,
  Header,
  HttpCode,
  Post,
  StreamableFile,
  UseGuards,
  UseInterceptors,
} from '@nestjs/common';
import { BearerGuard } from './bearer.guard.js';
import { EchoDto } from './echo.dto.js';
import { HelloService } from './hello.service.js';
import { PassInterceptor } from './pass.interceptor.js';

@Controller()
export class HelloController {
  constructor(private readonly helloService: HelloService) {}

  @Get('ping')
  @Header('Content-Type', 'text/plain; charset=utf-8')
  ping(): string {
    return 'pong';
  }

  @Get('hello')
  hello(): { message: string } {
    return { message: this.helloService.greeting() };
  }

  @Get('layered')
  @UseGuards(BearerGuard)
  @UseInterceptors(PassInterceptor)
  layered(): { message: string } {
    return { message: this.helloService.greeting() };
  }

  @Get('stream')
  stream(): StreamableFile {
    const chunks = Array.from({ length: 16 }, () => Buffer.from('chunk\n'));
    return new StreamableFile(Readable.from(chunks), { type: 'text/plain' });
  }

  @Post('echo')
  @HttpCode(200)
  echo(@Body() body: EchoDto): EchoDto {
    return body;
  }

  @Get('large')
  large(): { id: number; message: string }[] {
    return this.helloService.items();
  }
}
