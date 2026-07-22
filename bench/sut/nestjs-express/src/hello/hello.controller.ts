import { Controller, Get, Header } from '@nestjs/common';
import { HelloService } from './hello.service';

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
}
